use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::domain::application::ApplicationName;
use crate::domain::identity::DeploymentId;
use crate::domain::reconciliation::{QuadletSourceObservation, SystemdUnitObservation};
use crate::domain::release::OciArtifact;
use crate::domain::runtime::{ContainerPort, HostPort, stable_runtime_name};

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
    ReadUnit {
        path: PathBuf,
        source: io::Error,
    },
    ObserveUnit {
        unit: String,
        stderr: String,
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
            Self::ReadUnit { path, source } => write!(
                formatter,
                "failed to read Quadlet unit {}: {source}",
                path.display()
            ),
            Self::ObserveUnit { unit, stderr } => write!(
                formatter,
                "systemctl failed while observing `{unit}`: {}",
                stderr.trim()
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
            | Self::ReadUnit { source, .. }
            | Self::Execute { source, .. } => Some(source),
            Self::HomeUnavailable | Self::ObserveUnit { .. } | Self::Systemd { .. } => None,
        }
    }
}

// Derives the stable Quadlet unit base name from the logical application and deployment identity.
pub fn unit_name(application_name: &ApplicationName, deployment_id: &DeploymentId) -> String {
    stable_runtime_name(application_name.as_str(), deployment_id.as_str())
}

// Keeps the generated Podman container name aligned with the Quadlet unit identity.
pub fn container_name(application_name: &ApplicationName, deployment_id: &DeploymentId) -> String {
    stable_runtime_name(application_name.as_str(), deployment_id.as_str())
}

// Renders the exact unit representation used for both materialization and reconciliation checks.
pub fn canonical_unit_contents(
    application_name: &ApplicationName,
    deployment_id: &DeploymentId,
    artifact: &OciArtifact,
    container_port: ContainerPort,
    host_port: HostPort,
) -> String {
    format!(
        "[Unit]\nDescription=Pneuma application {}\n\n[Container]\nContainerName={}\nImage={}\nPublishPort=127.0.0.1:{}:{}\nLabel=io.pneuma.application={}\nLabel=io.pneuma.image-digest={}\n\n[Service]\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        application_name.as_str(),
        container_name(application_name, deployment_id),
        artifact.reference(),
        host_port.get(),
        container_port.get(),
        application_name.as_str(),
        artifact.digest(),
    )
}

// Writes a rootless, loopback-bound Quadlet unit that systemd can recreate after Pneuma exits.
pub fn write_unit(
    application_name: &ApplicationName,
    deployment_id: &DeploymentId,
    artifact: &OciArtifact,
    container_port: ContainerPort,
    host_port: HostPort,
) -> Result<String, QuadletError> {
    let unit = unit_name(application_name, deployment_id);
    let directory = quadlet_directory()?;
    fs::create_dir_all(&directory).map_err(|source| QuadletError::CreateDirectory { source })?;
    let path = directory.join(format!("{unit}.container"));
    let content = canonical_unit_contents(
        application_name,
        deployment_id,
        artifact,
        container_port,
        host_port,
    );
    fs::write(&path, content).map_err(|source| QuadletError::WriteUnit { path, source })?;
    Ok(unit)
}

// Removes a generated Quadlet file idempotently so candidate cleanup can be retried safely.
pub fn remove_unit(unit: &str) -> Result<(), QuadletError> {
    let path = quadlet_directory()?.join(format!("{unit}.container"));
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(QuadletError::RemoveUnit { path, source }),
    }
}

// Reports whether the expected Quadlet source file remains available for runtime recovery.
pub fn unit_exists(unit: &str) -> Result<bool, QuadletError> {
    Ok(quadlet_directory()?
        .join(format!("{unit}.container"))
        .exists())
}

// Reads the source Quadlet without creating its directory or changing user-systemd state.
pub fn observe_unit_source(unit: &str) -> Result<QuadletSourceObservation, QuadletError> {
    let path = quadlet_directory()?.join(format!("{unit}.container"));
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(QuadletSourceObservation::Present { contents }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            Ok(QuadletSourceObservation::Missing)
        }
        Err(source) => Err(QuadletError::ReadUnit { path, source }),
    }
}

// Reads systemd's generated-unit state without starting, stopping, or reloading it.
pub fn observe_generated_unit(unit: &str) -> Result<SystemdUnitObservation, QuadletError> {
    let service = format!("{unit}.service");
    let output = Command::new("systemctl")
        .args(["--user", "is-active", &service])
        .output()
        .map_err(|source| QuadletError::Execute {
            operation: "observing generated unit",
            source,
        })?;
    if output.status.code() == Some(4) {
        return Ok(SystemdUnitObservation::Missing);
    }
    if !(output.status.success() || output.status.code() == Some(3)) {
        return Err(QuadletError::ObserveUnit {
            unit: service,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let active_state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(SystemdUnitObservation::Present { active_state })
}

// Regenerates user-systemd units after a Quadlet file changes.
pub fn daemon_reload() -> Result<(), QuadletError> {
    control("reloading units", "", &["daemon-reload"])
}
// Starts the generated user service for a logical Quadlet unit.
pub fn start(unit: &str) -> Result<(), QuadletError> {
    control("starting", unit, &["start"])
}
// Stops the generated user service without removing its Quadlet definition.
pub fn stop(unit: &str) -> Result<(), QuadletError> {
    control("stopping", unit, &["stop"])
}

// Resolves the configurable rootless Quadlet directory, defaulting under the current user's home.
fn quadlet_directory() -> Result<PathBuf, QuadletError> {
    if let Some(directory) =
        env::var_os(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE).filter(|path| !path.is_empty())
    {
        return Ok(PathBuf::from(directory));
    }
    let home = env::var_os("HOME").ok_or(QuadletError::HomeUnavailable)?;
    Ok(Path::new(&home).join(".config/containers/systemd"))
}

// Invokes systemctl --user and preserves the service-specific failure diagnostic.
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
