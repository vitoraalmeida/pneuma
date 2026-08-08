use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE: &str = "PNEUMA_QUADLET_DIR";

#[derive(Debug)]
pub enum QuadletError {
    HomeUnavailable,
    CreateDirectory {
        source: io::Error,
    },
    WriteUnit {
        path: PathBuf,
        source: io::Error,
    },
    RemoveUnit {
        path: PathBuf,
        source: io::Error,
    },
    Execute {
        operation: &'static str,
        source: io::Error,
    },
    Systemd {
        operation: &'static str,
        unit: String,
        stderr: String,
    },
}

impl fmt::Display for QuadletError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeUnavailable => {
                formatter.write_str("HOME is required to locate the Quadlet directory")
            }
            Self::CreateDirectory { source } => {
                write!(formatter, "failed to create Quadlet directory: {source}")
            }
            Self::WriteUnit { path, source } => write!(
                formatter,
                "failed to write Quadlet unit {}: {source}",
                path.display()
            ),
            Self::RemoveUnit { path, source } => write!(
                formatter,
                "failed to remove Quadlet unit {}: {source}",
                path.display()
            ),
            Self::Execute { operation, source } => write!(
                formatter,
                "failed to execute systemctl while {operation}: {source}"
            ),
            Self::Systemd {
                operation,
                unit,
                stderr,
            } => write!(
                formatter,
                "systemctl failed while {operation} `{unit}`: {}",
                stderr.trim()
            ),
        }
    }
}

impl Error for QuadletError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CreateDirectory { source }
            | Self::WriteUnit { source, .. }
            | Self::RemoveUnit { source, .. }
            | Self::Execute { source, .. } => Some(source),
            Self::HomeUnavailable | Self::Systemd { .. } => None,
        }
    }
}

pub fn unit_name(application_name: &str, deployment_id: &str) -> String {
    format!("pneuma-{application_name}-{deployment_id}")
}

pub fn container_name(application_name: &str, deployment_id: &str) -> String {
    format!("pneuma-{application_name}-{deployment_id}")
}

pub fn write_unit(
    application_name: &str,
    deployment_id: &str,
    image_reference: &str,
    container_port: u16,
    host_port: u16,
    revision: &str,
) -> Result<String, QuadletError> {
    let unit = unit_name(application_name, deployment_id);
    let directory = quadlet_directory()?;
    fs::create_dir_all(&directory).map_err(|source| QuadletError::CreateDirectory { source })?;
    let path = directory.join(format!("{unit}.container"));
    let content = format!(
        "[Unit]\nDescription=Pneuma application {application_name}\n\n[Container]\nContainerName={}\nImage={image_reference}\nPublishPort=127.0.0.1:{host_port}:{container_port}\nLabel=io.pneuma.application={application_name}\nLabel=io.pneuma.revision={revision}\n\n[Service]\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        container_name(application_name, deployment_id),
    );
    fs::write(&path, content).map_err(|source| QuadletError::WriteUnit { path, source })?;
    Ok(unit)
}

pub fn remove_unit(unit: &str) -> Result<(), QuadletError> {
    let path = quadlet_directory()?.join(format!("{unit}.container"));
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(QuadletError::RemoveUnit { path, source }),
    }
}

pub fn unit_exists(unit: &str) -> Result<bool, QuadletError> {
    Ok(quadlet_directory()?
        .join(format!("{unit}.container"))
        .exists())
}

pub fn daemon_reload() -> Result<(), QuadletError> {
    control("reloading units", "", &["daemon-reload"])
}
pub fn start(unit: &str) -> Result<(), QuadletError> {
    control("starting", unit, &["start"])
}
pub fn stop(unit: &str) -> Result<(), QuadletError> {
    control("stopping", unit, &["stop"])
}

fn quadlet_directory() -> Result<PathBuf, QuadletError> {
    if let Some(directory) =
        env::var_os(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE).filter(|path| !path.is_empty())
    {
        return Ok(PathBuf::from(directory));
    }
    let home = env::var_os("HOME").ok_or(QuadletError::HomeUnavailable)?;
    Ok(Path::new(&home).join(".config/containers/systemd"))
}

fn control(operation: &'static str, unit: &str, arguments: &[&str]) -> Result<(), QuadletError> {
    let service = if unit.is_empty() {
        String::new()
    } else {
        format!("{unit}.service")
    };
    let mut command = Command::new("systemctl");
    command.arg("--user").args(arguments);
    if !service.is_empty() {
        command.arg(&service);
    }
    let output = command
        .output()
        .map_err(|source| QuadletError::Execute { operation, source })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(QuadletError::Systemd {
            operation,
            unit: service,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}
