use std::error::Error;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::process::Command;

use crate::domain::reconciliation::NamedContainerObservation;
use crate::domain::runtime::{
    ContainerId, ContainerObservation, ContainerPort, ObservedRuntimeState,
    validate_loopback_endpoint,
};

#[derive(Debug, PartialEq, Eq)]
// Preserves Podman diagnostics from successful lifecycle commands for callers that report effects.
pub(crate) struct ContainerCommandOutput {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[derive(Debug)]
pub enum ControlContainerError {
    InvalidContainerId,
    Execute {
        operation: &'static str,
        source: io::Error,
    },
    Podman {
        operation: &'static str,
        container_id: String,
        stdout: String,
        stderr: String,
    },
}

#[derive(Debug)]
pub enum ObserveContainerError {
    Execute {
        operation: &'static str,
        source: io::Error,
    },
    Podman {
        operation: &'static str,
        container_id: String,
        stdout: String,
        stderr: String,
    },
    InvalidState {
        container_id: String,
    },
    InvalidEndpoint {
        container_id: String,
        output: String,
    },
}

#[derive(Debug)]
pub(crate) enum ResolveContainerError {
    EmptyName,
    Execute {
        source: io::Error,
    },
    Podman {
        name: String,
        stdout: String,
        stderr: String,
    },
    InvalidOutput {
        name: String,
    },
}

#[derive(Debug)]
pub enum ObserveNamedContainerError {
    EmptyName,
    Execute {
        operation: &'static str,
        source: io::Error,
    },
    Podman {
        operation: &'static str,
        name: String,
        stdout: String,
        stderr: String,
    },
    InvalidOutput {
        name: String,
    },
    Observe {
        source: ObserveContainerError,
    },
}

impl fmt::Display for ControlContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContainerId => {
                formatter.write_str("container ID must be a non-empty hexadecimal value")
            }
            Self::Execute { operation, source } => {
                write!(
                    formatter,
                    "failed to execute Podman while {operation}: {source}"
                )
            }
            Self::Podman {
                operation,
                container_id,
                stdout,
                stderr,
            } => write!(
                formatter,
                "Podman failed while {operation} container `{container_id}`: {}",
                diagnostic(stdout, stderr)
            ),
        }
    }
}

impl Error for ControlContainerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source, .. } => Some(source),
            Self::InvalidContainerId | Self::Podman { .. } => None,
        }
    }
}

impl fmt::Display for ObserveContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Execute { operation, source } => {
                write!(
                    formatter,
                    "failed to execute Podman while {operation}: {source}"
                )
            }
            Self::Podman {
                operation,
                container_id,
                stdout,
                stderr,
            } => write!(
                formatter,
                "Podman failed while {operation} container `{container_id}`: {}",
                diagnostic(stdout, stderr)
            ),
            Self::InvalidState { container_id } => write!(
                formatter,
                "Podman returned an empty state for container `{container_id}`"
            ),
            Self::InvalidEndpoint {
                container_id,
                output,
            } => write!(
                formatter,
                "Podman returned an invalid loopback endpoint for container `{container_id}`: {output}"
            ),
        }
    }
}

impl Error for ObserveContainerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source, .. } => Some(source),
            Self::Podman { .. } | Self::InvalidState { .. } | Self::InvalidEndpoint { .. } => None,
        }
    }
}

impl fmt::Display for ResolveContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => formatter.write_str("container name must not be empty"),
            Self::Execute { source } => write!(formatter, "failed to execute Podman: {source}"),
            Self::Podman {
                name,
                stdout,
                stderr,
            } => write!(
                formatter,
                "failed to resolve container `{name}` with Podman: {}",
                diagnostic(stdout, stderr)
            ),
            Self::InvalidOutput { name } => write!(
                formatter,
                "Podman returned an invalid ID for container `{name}`"
            ),
        }
    }
}

impl Error for ResolveContainerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source } => Some(source),
            Self::EmptyName | Self::Podman { .. } | Self::InvalidOutput { .. } => None,
        }
    }
}

impl fmt::Display for ObserveNamedContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => formatter.write_str("container name must not be empty"),
            Self::Execute { operation, source } => write!(
                formatter,
                "failed to execute Podman while {operation}: {source}"
            ),
            Self::Podman {
                operation,
                name,
                stdout,
                stderr,
            } => write!(
                formatter,
                "Podman failed while {operation} container `{name}`: {}",
                diagnostic(stdout, stderr)
            ),
            Self::InvalidOutput { name } => write!(
                formatter,
                "Podman returned invalid materialization data for container `{name}`"
            ),
            Self::Observe { source } => {
                write!(formatter, "failed to observe resolved container: {source}")
            }
        }
    }
}

impl Error for ObserveNamedContainerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source, .. } => Some(source),
            Self::Observe { source } => Some(source),
            Self::EmptyName | Self::Podman { .. } | Self::InvalidOutput { .. } => None,
        }
    }
}

// Starts a validated container through the shared Podman lifecycle command path.
pub(crate) fn start_container(
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("starting", &["start"], container_id)
}

// Resolves Podman's current container ID by stable name because recreation changes external IDs.
pub(crate) fn resolve_container_id(name: &str) -> Result<ContainerId, ResolveContainerError> {
    if name.is_empty() {
        return Err(ResolveContainerError::EmptyName);
    }
    let output = Command::new("podman")
        .args(["inspect", "--format", "{{.Id}}", name])
        .output()
        .map_err(|source| ResolveContainerError::Execute { source })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(ResolveContainerError::Podman {
            name: name.to_owned(),
            stdout,
            stderr,
        });
    }
    let id = stdout.trim();
    if !ContainerId::is_valid(id) {
        return Err(ResolveContainerError::InvalidOutput {
            name: name.to_owned(),
        });
    }
    Ok(ContainerId::from(id.to_owned()))
}

// Observes a deterministic container name without treating ordinary absence as an adapter failure.
pub(crate) fn observe_named_container(
    name: &str,
    container_port: ContainerPort,
) -> Result<NamedContainerObservation, ObserveNamedContainerError> {
    if name.is_empty() {
        return Err(ObserveNamedContainerError::EmptyName);
    }
    let exists = Command::new("podman")
        .args(["container", "exists", name])
        .output()
        .map_err(|source| ObserveNamedContainerError::Execute {
            operation: "checking for",
            source,
        })?;
    if exists.status.code() == Some(1) {
        return Ok(NamedContainerObservation::Missing);
    }
    if !exists.status.success() {
        return Err(named_container_failure("checking for", name, exists));
    }
    let format = "{{.Id}}\t{{.Name}}\t{{.Config.Image}}\t{{index .Config.Labels \"io.pneuma.application\"}}\t{{index .Config.Labels \"io.pneuma.image-digest\"}}";
    let inspected = Command::new("podman")
        .args(["inspect", "--format", format, name])
        .output()
        .map_err(|source| ObserveNamedContainerError::Execute {
            operation: "inspecting",
            source,
        })?;
    if !inspected.status.success() {
        return Err(named_container_failure("inspecting", name, inspected));
    }
    let output = String::from_utf8_lossy(&inspected.stdout);
    let mut values = output.trim().split('\t');
    let (
        Some(id),
        Some(observed_name),
        Some(image_reference),
        Some(application_label),
        Some(image_digest_label),
        None,
    ) = (
        values.next(),
        values.next(),
        values.next(),
        values.next(),
        values.next(),
        values.next(),
    )
    else {
        return Err(ObserveNamedContainerError::InvalidOutput {
            name: name.to_owned(),
        });
    };
    let id = if ContainerId::is_valid(id) {
        ContainerId::from(id.to_owned())
    } else {
        return Err(ObserveNamedContainerError::InvalidOutput {
            name: name.to_owned(),
        });
    };
    if observed_name.is_empty() || image_reference.is_empty() {
        return Err(ObserveNamedContainerError::InvalidOutput {
            name: name.to_owned(),
        });
    }
    let observation = observe_container(&id, container_port)
        .map_err(|source| ObserveNamedContainerError::Observe { source })?;
    Ok(NamedContainerObservation::Present {
        id,
        name: observed_name.to_owned(),
        image_reference: image_reference.to_owned(),
        application_label: (!application_label.is_empty()).then(|| application_label.to_owned()),
        image_digest_label: (!image_digest_label.is_empty()).then(|| image_digest_label.to_owned()),
        observation,
    })
}

// Stops a validated container through the shared Podman lifecycle command path.
pub(crate) fn stop_container(
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("stopping", &["stop"], container_id)
}

// Force-removes a validated candidate container during cleanup after a failed deployment.
pub(crate) fn remove_container(
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("removing", &["container", "rm", "--force"], container_id)
}

// Observes container state and exposes an endpoint only while Podman confirms it is running.
pub(crate) fn observe_container(
    container_id: &ContainerId,
    container_port: ContainerPort,
) -> Result<ContainerObservation, ObserveContainerError> {
    let exists = Command::new("podman")
        .args(["container", "exists", container_id.as_str()])
        .output()
        .map_err(|source| ObserveContainerError::Execute {
            operation: "checking for",
            source,
        })?;
    if exists.status.code() == Some(1) {
        return Ok(ContainerObservation::missing());
    }
    if !exists.status.success() {
        return Err(observation_failure(
            "checking for",
            container_id.as_str(),
            exists,
        ));
    }

    let status = Command::new("podman")
        .args([
            "inspect",
            "--format",
            "{{.State.Status}}",
            container_id.as_str(),
        ])
        .output()
        .map_err(|source| ObserveContainerError::Execute {
            operation: "observing",
            source,
        })?;
    if !status.status.success() {
        return Err(observation_failure(
            "observing",
            container_id.as_str(),
            status,
        ));
    }
    let status = String::from_utf8_lossy(&status.stdout).trim().to_owned();
    if status.is_empty() {
        return Err(ObserveContainerError::InvalidState {
            container_id: container_id.as_str().to_owned(),
        });
    }
    let state = observed_state(&status);
    if state == ObservedRuntimeState::Running {
        let endpoint = observe_endpoint(container_id.as_str(), container_port.get())?;
        ContainerObservation::running(endpoint).map_err(|_| {
            ObserveContainerError::InvalidEndpoint {
                container_id: container_id.as_str().to_owned(),
                output: "Podman returned a non-loopback endpoint".to_owned(),
            }
        })
    } else {
        ContainerObservation::not_running(state).map_err(|_| ObserveContainerError::InvalidState {
            container_id: container_id.as_str().to_owned(),
        })
    }
}

// Executes a lifecycle command after validating the external container ID used as its target.
fn control_container(
    operation: &'static str,
    arguments: &[&str],
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    if !ContainerId::is_valid(container_id) {
        return Err(ControlContainerError::InvalidContainerId);
    }

    let output = Command::new("podman")
        .args(arguments)
        .arg(container_id)
        .output()
        .map_err(|source| ControlContainerError::Execute { operation, source })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(ControlContainerError::Podman {
            operation,
            container_id: container_id.to_owned(),
            stdout,
            stderr,
        });
    }

    Ok(ContainerCommandOutput { stdout, stderr })
}

fn named_container_failure(
    operation: &'static str,
    name: &str,
    output: std::process::Output,
) -> ObserveNamedContainerError {
    ObserveNamedContainerError::Podman {
        operation,
        name: name.to_owned(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

// Reads Podman's published endpoint and accepts only the loopback binding required by the runtime boundary.
fn observe_endpoint(
    container_id: &str,
    container_port: u16,
) -> Result<SocketAddr, ObserveContainerError> {
    let port = format!("{container_port}/tcp");
    let output = Command::new("podman")
        .args(["port", container_id, &port])
        .output()
        .map_err(|source| ObserveContainerError::Execute {
            operation: "observing the endpoint of",
            source,
        })?;
    if !output.status.success() {
        return Err(observation_failure(
            "observing the endpoint of",
            container_id,
            output,
        ));
    }

    let output = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let endpoint = output
        .lines()
        .next()
        .and_then(|line| line.parse::<SocketAddr>().ok())
        .filter(|endpoint| validate_loopback_endpoint(*endpoint).is_ok())
        .ok_or_else(|| ObserveContainerError::InvalidEndpoint {
            container_id: container_id.to_owned(),
            output,
        })?;
    Ok(endpoint)
}

// Converts a failed Podman observation into diagnostics tied to the attempted operation and container.
fn observation_failure(
    operation: &'static str,
    container_id: &str,
    output: std::process::Output,
) -> ObserveContainerError {
    ObserveContainerError::Podman {
        operation,
        container_id: container_id.to_owned(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

// Maps Podman's open-ended state strings into the closed domain observation set.
fn observed_state(status: &str) -> ObservedRuntimeState {
    match status {
        "configured" | "created" => ObservedRuntimeState::Created,
        "initialized" => ObservedRuntimeState::Starting,
        "running" => ObservedRuntimeState::Running,
        "stopping" | "removing" => ObservedRuntimeState::Stopping,
        "stopped" | "exited" => ObservedRuntimeState::Stopped,
        "dead" => ObservedRuntimeState::Failed,
        status => ObservedRuntimeState::Unknown {
            status: status.to_owned(),
        },
    }
}

// Prefers Podman's stderr failure detail, using stdout only when stderr is empty.
fn diagnostic<'a>(stdout: &'a str, stderr: &'a str) -> &'a str {
    if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn maps_podman_states_to_explicit_runtime_states() {
        let cases = [
            ("configured", ObservedRuntimeState::Created),
            ("initialized", ObservedRuntimeState::Starting),
            ("running", ObservedRuntimeState::Running),
            ("stopping", ObservedRuntimeState::Stopping),
            ("exited", ObservedRuntimeState::Stopped),
            ("dead", ObservedRuntimeState::Failed),
            (
                "paused",
                ObservedRuntimeState::Unknown {
                    status: "paused".to_owned(),
                },
            ),
        ];

        for (status, expected) in cases {
            assert_eq!(observed_state(status), expected);
        }
    }

    // Fake `podman` used by the adapter contract tests below. Every invocation
    // is logged as one argv line; behavior is selected per subcommand through
    // PNEUMA_FAKE_PODMAN_* variables.
    const FAKE_PODMAN: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_PODMAN_LOG\"
if [ \"$1\" = \"container\" ] && [ \"$2\" = \"exists\" ]; then
  exit \"${PNEUMA_FAKE_PODMAN_EXISTS:-1}\"
fi
if [ \"$1\" = \"port\" ]; then
  printf '%s\\n' \"${PNEUMA_FAKE_PODMAN_PORT:-127.0.0.1:31000}\"
  exit 0
fi
if [ \"$1\" = \"inspect\" ]; then
  case \"$3\" in
    \"{{.Id}}\")
      printf '%s\\n' \"${PNEUMA_FAKE_PODMAN_ID:-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}\";;
    \"{{.State.Status}}\")
      printf '%s\\n' \"${PNEUMA_FAKE_PODMAN_STATUS:-running}\";;
    *)
      printf '%s\\n' \"$PNEUMA_FAKE_PODMAN_NAMED\";;
  esac
  exit \"${PNEUMA_FAKE_PODMAN_INSPECT_EXIT:-0}\"
fi
exit \"${PNEUMA_FAKE_PODMAN_EXIT:-0}\"
";

    fn container_id(character: char) -> String {
        character.to_string().repeat(64)
    }

    struct ScopedPodman {
        _path: crate::test_support::ScopedExternalPath,
        log: PathBuf,
    }

    impl ScopedPodman {
        // Tests holding the shared external-PATH lock never run concurrently,
        // so stale behavior variables from an earlier scenario can be cleared.
        const BEHAVIOR_VARIABLES: [&str; 6] = [
            "PNEUMA_FAKE_PODMAN_EXISTS",
            "PNEUMA_FAKE_PODMAN_PORT",
            "PNEUMA_FAKE_PODMAN_ID",
            "PNEUMA_FAKE_PODMAN_STATUS",
            "PNEUMA_FAKE_PODMAN_NAMED",
            "PNEUMA_FAKE_PODMAN_INSPECT_EXIT",
        ];

        fn new(name: &str) -> Self {
            let path =
                crate::test_support::ScopedExternalPath::new(name, &[("podman", FAKE_PODMAN)]);
            for variable in Self::BEHAVIOR_VARIABLES {
                path.remove_var(variable);
            }
            path.remove_var("PNEUMA_FAKE_PODMAN_EXIT");
            let log = path.directory().join("invocations.log");
            path.set_var("PNEUMA_FAKE_PODMAN_LOG", &log.to_string_lossy());
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

    #[test]
    fn control_commands_target_the_recorded_container_and_map_failures() {
        let scoped = ScopedPodman::new("control");
        let id = container_id('a');

        assert!(matches!(
            start_container("not-hex"),
            Err(ControlContainerError::InvalidContainerId)
        ));
        assert!(scoped.invocations().is_empty());

        start_container(&id).unwrap();
        stop_container(&id).unwrap();
        remove_container(&id).unwrap();

        assert_eq!(
            scoped.invocations(),
            [
                format!("start {id}"),
                format!("stop {id}"),
                format!("container rm --force {id}"),
            ]
        );

        scoped._path.set_var("PNEUMA_FAKE_PODMAN_EXIT", "9");
        let error = stop_container(&id).unwrap_err();
        assert!(matches!(
            error,
            ControlContainerError::Podman {
                operation: "stopping",
                ..
            }
        ));
    }

    #[test]
    fn observe_container_reports_missing_without_inspecting() {
        let scoped = ScopedPodman::new("observe-missing");
        let id = ContainerId::from(container_id('b'));

        let observation = observe_container(&id, ContainerPort::new(8080).unwrap()).unwrap();

        assert_eq!(observation, ContainerObservation::missing());
        assert_eq!(
            scoped.invocations(),
            [format!("container exists {}", id.as_str())]
        );
    }

    #[test]
    fn observe_container_maps_running_unknown_and_foreign_endpoint_states() {
        let scoped = ScopedPodman::new("observe-states");
        let id = ContainerId::from(container_id('c'));
        scoped._path.set_var("PNEUMA_FAKE_PODMAN_EXISTS", "0");

        let observation = observe_container(&id, ContainerPort::new(8080).unwrap()).unwrap();
        assert_eq!(
            observation,
            ContainerObservation::Running {
                observed_endpoint: "127.0.0.1:31000".parse().unwrap(),
            }
        );

        scoped._path.set_var("PNEUMA_FAKE_PODMAN_STATUS", "paused");
        let observation = observe_container(&id, ContainerPort::new(8080).unwrap()).unwrap();
        assert_eq!(
            observation,
            ContainerObservation::NotRunning {
                state: ObservedRuntimeState::Unknown {
                    status: "paused".to_owned(),
                },
            }
        );

        scoped._path.set_var("PNEUMA_FAKE_PODMAN_STATUS", "running");
        scoped
            ._path
            .set_var("PNEUMA_FAKE_PODMAN_PORT", "10.0.0.2:31000");
        assert!(matches!(
            observe_container(&id, ContainerPort::new(8080).unwrap()),
            Err(ObserveContainerError::InvalidEndpoint { .. })
        ));
    }

    #[test]
    fn resolve_container_id_validates_podmans_answer() {
        let scoped = ScopedPodman::new("resolve");

        scoped
            ._path
            .set_var("PNEUMA_FAKE_PODMAN_ID", "not a container id");
        let error = resolve_container_id("pneuma-app-1").unwrap_err();
        assert!(matches!(error, ResolveContainerError::InvalidOutput { .. }));

        scoped
            ._path
            .set_var("PNEUMA_FAKE_PODMAN_ID", &container_id('d'));
        let resolved = resolve_container_id("pneuma-app-1").unwrap();
        assert_eq!(resolved.as_str(), container_id('d'));

        scoped._path.set_var("PNEUMA_FAKE_PODMAN_INSPECT_EXIT", "1");
        assert!(matches!(
            resolve_container_id("pneuma-app-1"),
            Err(ResolveContainerError::Podman { .. })
        ));
    }
    #[test]
    fn observe_named_container_parses_identity_labels_and_preserves_absence() {
        let scoped = ScopedPodman::new("observe-named");
        let id = container_id('e');
        let named = format!(
            "{}\tpneuma-app\tregistry.example/app@sha256:{}\tmyapp\tsha256:{}",
            id,
            container_id('f'),
            container_id('g'),
        );
        scoped._path.set_var("PNEUMA_FAKE_PODMAN_EXISTS", "0");
        scoped._path.set_var("PNEUMA_FAKE_PODMAN_NAMED", &named);

        let observation =
            observe_named_container("pneuma-app-1", ContainerPort::new(8080).unwrap()).unwrap();
        match observation {
            NamedContainerObservation::Present {
                id: observed_id,
                name,
                image_reference,
                application_label,
                image_digest_label,
                observation,
            } => {
                assert_eq!(observed_id.as_str(), id);
                assert_eq!(name, "pneuma-app");
                assert_eq!(
                    image_reference,
                    format!("registry.example/app@sha256:{}", container_id('f'))
                );
                assert_eq!(application_label.as_deref(), Some("myapp"));
                assert_eq!(
                    image_digest_label.as_deref(),
                    Some(format!("sha256:{}", container_id('g')).as_str())
                );
                assert!(matches!(observation, ContainerObservation::Running { .. }));
            }
            other => panic!("expected a present observation, got {other:?}"),
        }

        // Trailing empty label fields are trimmed away, so a present container
        // without its identity labels is refused instead of adopted with
        // invented or partially-absent identity (conservative external boundary).
        let unnamed = format!(
            "{}\tpneuma-app\tregistry.example/app@sha256:{}\t\t",
            id,
            container_id('f'),
        );
        scoped._path.set_var("PNEUMA_FAKE_PODMAN_NAMED", &unnamed);
        assert!(matches!(
            observe_named_container("pneuma-app-1", ContainerPort::new(8080).unwrap()),
            Err(ObserveNamedContainerError::InvalidOutput { .. })
        ));

        scoped._path.remove_var("PNEUMA_FAKE_PODMAN_EXISTS");
        assert!(matches!(
            observe_named_container("pneuma-app-9", ContainerPort::new(8080).unwrap()),
            Ok(NamedContainerObservation::Missing)
        ));

        assert!(matches!(
            observe_named_container("", ContainerPort::new(8080).unwrap()),
            Err(ObserveNamedContainerError::EmptyName)
        ));
    }
}
