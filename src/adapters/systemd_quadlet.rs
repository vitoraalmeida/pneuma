use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use crate::domain::application::ApplicationName;
use crate::domain::identity::DeploymentId;
use crate::domain::reconciliation::{QuadletSourceObservation, SystemdUnitObservation};
use crate::domain::release::OciArtifact;
use crate::domain::runtime::{ContainerPort, HostPort, stable_runtime_name};

pub(crate) const QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE: &str = "PNEUMA_QUADLET_DIR";

#[derive(Debug, Error)]
pub enum QuadletError {
    #[error("HOME is required to locate the Quadlet directory")]
    HomeUnavailable,
    #[error("failed to create Quadlet directory: {source}")]
    CreateDirectory {
        #[source]
        source: io::Error,
    },
    #[error("failed to write Quadlet unit {}: {source}", path.display())]
    WriteUnit {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to remove Quadlet unit {}: {source}", path.display())]
    RemoveUnit {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read Quadlet unit {}: {source}", path.display())]
    ReadUnit {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("systemctl failed while observing `{unit}`: {}", stderr.trim())]
    ObserveUnit { unit: String, stderr: String },
    #[error("failed to execute systemctl while {operation}: {source}")]
    Execute {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("systemctl failed while {operation} `{unit}`: {}", stderr.trim())]
    Systemd {
        operation: &'static str,
        unit: String,
        stderr: String,
    },
    #[error("systemctl failed while reloading user-systemd units: {}", stderr.trim())]
    ReloadUnits { stderr: String },
}

// Derives the stable Quadlet unit base name from the logical application and deployment identity.
pub(crate) fn unit_name(
    application_name: &ApplicationName,
    deployment_id: &DeploymentId,
) -> String {
    stable_runtime_name(application_name.as_str(), deployment_id.as_str())
}

// Keeps the generated Podman container name aligned with the Quadlet unit identity.
pub(crate) fn container_name(
    application_name: &ApplicationName,
    deployment_id: &DeploymentId,
) -> String {
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
pub(crate) fn write_unit(
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

// Removes a generated Quadlet file idempotently so candidate cleanup can be retried safely.
pub(crate) fn remove_unit(unit: &str) -> Result<(), QuadletError> {
    let path = quadlet_directory()?.join(format!("{unit}.container"));
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(QuadletError::RemoveUnit { path, source }),
    }
}

// Reports whether the expected Quadlet source file remains available for runtime recovery.
pub(crate) fn unit_exists(unit: &str) -> Result<bool, QuadletError> {
    Ok(quadlet_directory()?
        .join(format!("{unit}.container"))
        .exists())
}

// Reads the source Quadlet without creating its directory or changing user-systemd state.
pub(crate) fn observe_unit_source(unit: &str) -> Result<QuadletSourceObservation, QuadletError> {
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
pub(crate) fn observe_generated_unit(unit: &str) -> Result<SystemdUnitObservation, QuadletError> {
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

// Regenerates user-systemd units after a Quadlet file changes; no unit is targeted.
pub(crate) fn daemon_reload() -> Result<(), QuadletError> {
    let output = Command::new("systemctl")
        .arg("--user")
        .arg("daemon-reload")
        .output()
        .map_err(|source| QuadletError::Execute {
            operation: "reloading units",
            source,
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(QuadletError::ReloadUnits {
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// Starts the generated user service for a logical Quadlet unit.
pub(crate) fn start(unit: &str) -> Result<(), QuadletError> {
    control("starting", unit, &["start"])
}

// Invokes systemctl --user for a generated service and preserves its failure diagnostic.
fn control(operation: &'static str, unit: &str, arguments: &[&str]) -> Result<(), QuadletError> {
    let service = format!("{unit}.service");
    let mut command = Command::new("systemctl");
    command.arg("--user").args(arguments).arg(&service);
    let output = command
        .output()
        .map_err(|source| QuadletError::Execute { operation, source })?;
    if output.status.success() {
        return Ok(());
    }
    Err(QuadletError::Systemd {
        operation,
        unit: service,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// Stops the generated user service without removing its Quadlet definition.
pub(crate) fn stop(unit: &str) -> Result<(), QuadletError> {
    control("stopping", unit, &["stop"])
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    // Env overrides are process-global, so quadlet tests serialize directory access
    // through the shared test-support lock.
    struct ScopedQuadletDirectory {
        _guard: std::sync::MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
        directory: PathBuf,
    }

    impl ScopedQuadletDirectory {
        fn new(name: &str) -> Self {
            let guard = crate::test_support::lock_quadlet_directory();
            let directory = std::env::temp_dir().join(format!(
                "pneuma-quadlet-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let previous = env::var_os(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE);
            // Safety: every access to PNEUMA_QUADLET_DIR in this process happens
            // while holding QUADLET_DIRECTORY_LOCK, which `guard` keeps alive.
            unsafe { env::set_var(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE, &directory) };
            Self {
                _guard: guard,
                previous,
                directory,
            }
        }
    }

    impl Drop for ScopedQuadletDirectory {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(previous) => {
                    // Safety: see ScopedQuadletDirectory::new.
                    unsafe { env::set_var(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE, previous) };
                }
                None => {
                    // Safety: see ScopedQuadletDirectory::new.
                    unsafe { env::remove_var(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE) };
                }
            }
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn write_unit_rewrites_canonical_bytes_so_updates_are_retry_safe() {
        let scoped = ScopedQuadletDirectory::new("write-retry");
        let (application_name, deployment_id) = unit_inputs();

        let unit = write_unit(
            &application_name,
            &deployment_id,
            &artifact(),
            ContainerPort::new(8080).unwrap(),
            HostPort::new(31000).unwrap(),
        )
        .unwrap();
        let path = scoped.directory.join(format!("{unit}.container"));
        let canonical = canonical_unit_contents(
            &application_name,
            &deployment_id,
            &artifact(),
            ContainerPort::new(8080).unwrap(),
            HostPort::new(31000).unwrap(),
        );

        assert_eq!(fs::read_to_string(&path).unwrap(), canonical);

        // A divergent on-disk unit is replaced by canonical bytes on the next write.
        fs::write(&path, "stale divergent contents").unwrap();
        write_unit(
            &application_name,
            &deployment_id,
            &artifact(),
            ContainerPort::new(8080).unwrap(),
            HostPort::new(31000).unwrap(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), canonical);

        assert!(unit_exists(&unit).unwrap());
    }

    fn unit_inputs() -> (ApplicationName, DeploymentId) {
        (
            ApplicationName::new("app").unwrap(),
            DeploymentId::new("22222222222222222222222222222222").unwrap(),
        )
    }

    fn artifact() -> OciArtifact {
        OciArtifact::parse("registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()
    }

    #[test]
    fn remove_unit_tolerates_missing_units_and_stays_safe_to_retry() {
        let scoped = ScopedQuadletDirectory::new("remove-retry");
        let (application_name, deployment_id) = unit_inputs();
        let unit = unit_name(&application_name, &deployment_id);
        let path = scoped.directory.join(format!("{unit}.container"));

        // Neither the directory nor the unit file needs to exist for cleanup to succeed.
        remove_unit(&unit).unwrap();
        assert!(!scoped.directory.exists());

        fs::create_dir_all(&scoped.directory).unwrap();
        fs::write(&path, "candidate unit").unwrap();
        remove_unit(&unit).unwrap();
        remove_unit(&unit).unwrap();

        assert!(!path.exists());
        assert!(!unit_exists(&unit).unwrap());
    }

    #[test]
    fn observe_unit_source_reports_missing_without_creating_the_directory() {
        let scoped = ScopedQuadletDirectory::new("observe-missing");
        let (application_name, deployment_id) = unit_inputs();
        let unit = unit_name(&application_name, &deployment_id);

        assert!(!unit_exists(&unit).unwrap());
        assert_eq!(
            observe_unit_source(&unit).unwrap(),
            QuadletSourceObservation::Missing
        );
        assert!(!scoped.directory.exists());
    }

    // Fake `systemctl` recording every invocation; behavior comes from
    // PNEUMA_FAKE_SYSTEMCTL_* variables.
    const FAKE_SYSTEMCTL: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_SYSTEMCTL_LOG\"
exit \"${PNEUMA_FAKE_SYSTEMCTL_EXIT:-0}\"
";

    struct ScopedSystemctl {
        _path: crate::test_support::ScopedExternalPath,
        log: PathBuf,
    }

    impl ScopedSystemctl {
        fn new(name: &str) -> Self {
            let path = crate::test_support::ScopedExternalPath::new(
                name,
                &[("systemctl", FAKE_SYSTEMCTL)],
            );
            path.remove_var("PNEUMA_FAKE_SYSTEMCTL_EXIT");
            let log = path.directory().join("invocations.log");
            path.set_var("PNEUMA_FAKE_SYSTEMCTL_LOG", &log.to_string_lossy());
            Self { _path: path, log }
        }

        fn invocations(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    // observe_generated_unit answers through the same fake with per-case exits,
    // so it gets its own scope helper returning stdout separately.
    #[test]
    fn control_invokes_user_systemctl_with_the_expected_service() {
        let scoped = ScopedSystemctl::new("control");
        let (application_name, deployment_id) = unit_inputs();
        let unit = unit_name(&application_name, &deployment_id);
        let service = format!("{unit}.service");

        daemon_reload().unwrap();
        start(&unit).unwrap();
        stop(&unit).unwrap();

        assert_eq!(
            scoped.invocations(),
            [
                "--user daemon-reload".to_owned(),
                format!("--user start {service}"),
                format!("--user stop {service}"),
            ]
        );

        scoped._path.set_var("PNEUMA_FAKE_SYSTEMCTL_EXIT", "4");
        assert!(matches!(
            start(&unit),
            Err(QuadletError::Systemd {
                operation: "starting",
                ..
            })
        ));
        // The global reload targets no unit, so its failure carries no unit either.
        assert!(matches!(
            daemon_reload(),
            Err(QuadletError::ReloadUnits { .. })
        ));
    }

    #[test]
    fn generated_unit_observation_maps_absence_and_inactive_states() {
        let _path = crate::test_support::ScopedExternalPath::new(
            "observe-generated",
            &[(
                "systemctl",
                "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_SYSTEMCTL_LOG\"
if [ -n \"$PNEUMA_FAKE_SYSTEMCTL_EXIT\" ]; then
  if [ \"$PNEUMA_FAKE_SYSTEMCTL_EXIT\" = \"3\" ]; then printf 'inactive\\n'; fi
  exit \"$PNEUMA_FAKE_SYSTEMCTL_EXIT\"
fi
printf '%s\\n' \"${PNEUMA_FAKE_SYSTEMCTL_STATE:-active}\"
exit 0
",
            )],
        );
        let log = _path.directory().join("invocations.log");
        _path.set_var("PNEUMA_FAKE_SYSTEMCTL_LOG", &log.to_string_lossy());
        _path.remove_var("PNEUMA_FAKE_SYSTEMCTL_EXIT");
        let (application_name, deployment_id) = unit_inputs();
        let service = format!("{}.service", unit_name(&application_name, &deployment_id));

        // Unit absence is systemd exit code 4 and maps to Missing, never an error.
        _path.set_var("PNEUMA_FAKE_SYSTEMCTL_EXIT", "4");
        assert_eq!(
            observe_generated_unit(&service).unwrap(),
            SystemdUnitObservation::Missing
        );

        // The documented not-running family stays observable (inactive reports exit 3).
        _path.set_var("PNEUMA_FAKE_SYSTEMCTL_EXIT", "3");
        assert_eq!(
            observe_generated_unit(&service).unwrap(),
            SystemdUnitObservation::Present {
                active_state: "inactive".to_owned()
            }
        );

        // Any other failure is a typed error carrying the diagnostic.
        _path.set_var("PNEUMA_FAKE_SYSTEMCTL_EXIT", "9");
        assert!(matches!(
            observe_generated_unit(&service),
            Err(QuadletError::ObserveUnit { .. })
        ));
    }
}
