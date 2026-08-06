use std::error::Error;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::Command;

const APPLICATION_LABEL: &str = "io.pneuma.application";
const REVISION_LABEL: &str = "io.pneuma.revision";
const ROLE_LABEL: &str = "io.pneuma.role";

#[derive(Debug, PartialEq, Eq)]
pub struct CreatedContainer {
    pub id: String,
    pub name: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ContainerCommandOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ObservedRuntimeState {
    Missing,
    Created,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
    Unknown { status: String },
}

#[derive(Debug, PartialEq, Eq)]
pub struct ContainerObservation {
    pub state: ObservedRuntimeState,
    pub endpoint: Option<SocketAddr>,
}

#[derive(Debug)]
pub enum CreateContainerError {
    InvalidPort,
    Execute {
        source: io::Error,
    },
    Create {
        name: String,
        stdout: String,
        stderr: String,
    },
    InvalidOutput {
        name: String,
    },
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
    InvalidContainerId,
    InvalidPort,
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

impl fmt::Display for CreateContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPort => formatter.write_str("container port must be between 1 and 65535"),
            Self::Execute { source } => write!(formatter, "failed to execute Podman: {source}"),
            Self::Create {
                name,
                stdout,
                stderr,
            } => write!(
                formatter,
                "failed to create container `{name}` with Podman: {}",
                diagnostic(stdout, stderr)
            ),
            Self::InvalidOutput { name } => write!(
                formatter,
                "Podman returned an invalid ID for created container `{name}`"
            ),
        }
    }
}

impl Error for CreateContainerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source } => Some(source),
            Self::InvalidPort | Self::Create { .. } | Self::InvalidOutput { .. } => None,
        }
    }
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
            Self::InvalidContainerId => {
                formatter.write_str("container ID must be a non-empty hexadecimal value")
            }
            Self::InvalidPort => formatter.write_str("container port must be between 1 and 65535"),
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
            Self::InvalidContainerId
            | Self::InvalidPort
            | Self::Podman { .. }
            | Self::InvalidState { .. }
            | Self::InvalidEndpoint { .. } => None,
        }
    }
}

pub fn create_container(
    image_reference: &str,
    application_name: &str,
    commit_sha: &str,
    container_port: u16,
) -> Result<CreatedContainer, CreateContainerError> {
    if container_port == 0 {
        return Err(CreateContainerError::InvalidPort);
    }

    let name = container_name(application_name, commit_sha);
    let application_label = format!("{APPLICATION_LABEL}={application_name}");
    let revision_label = format!("{REVISION_LABEL}={commit_sha}");
    let role_label = format!("{ROLE_LABEL}=candidate");
    // Let Podman choose an unused host port, but constrain the mapping to loopback so a
    // candidate cannot become publicly reachable before health checks and promotion.
    let port_mapping = format!("127.0.0.1::{container_port}");
    let output = Command::new("podman")
        .args(["create", "--pull=never", "--name"])
        .arg(&name)
        .arg("--label")
        .arg(application_label)
        .arg("--label")
        .arg(revision_label)
        .arg("--label")
        .arg(role_label)
        .arg("--publish")
        .arg(port_mapping)
        .arg(image_reference)
        .output()
        .map_err(|source| CreateContainerError::Execute { source })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        return Err(CreateContainerError::Create {
            name,
            stdout,
            stderr,
        });
    }

    let id = stdout.trim();
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CreateContainerError::InvalidOutput { name });
    }

    Ok(CreatedContainer {
        id: id.to_owned(),
        name,
        stdout,
        stderr,
    })
}

pub fn start_container(
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("starting", &["start"], container_id)
}

pub fn stop_container(container_id: &str) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("stopping", &["stop"], container_id)
}

pub fn remove_container(
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("removing", &["container", "rm", "--force"], container_id)
}

pub fn observe_container(
    container_id: &str,
    container_port: u16,
) -> Result<ContainerObservation, ObserveContainerError> {
    if !is_container_id(container_id) {
        return Err(ObserveContainerError::InvalidContainerId);
    }
    if container_port == 0 {
        return Err(ObserveContainerError::InvalidPort);
    }

    let exists = Command::new("podman")
        .args(["container", "exists", container_id])
        .output()
        .map_err(|source| ObserveContainerError::Execute {
            operation: "checking for",
            source,
        })?;
    if exists.status.code() == Some(1) {
        return Ok(ContainerObservation {
            state: ObservedRuntimeState::Missing,
            endpoint: None,
        });
    }
    if !exists.status.success() {
        return Err(observation_failure("checking for", container_id, exists));
    }

    let status = Command::new("podman")
        .args(["inspect", "--format", "{{.State.Status}}", container_id])
        .output()
        .map_err(|source| ObserveContainerError::Execute {
            operation: "observing",
            source,
        })?;
    if !status.status.success() {
        return Err(observation_failure("observing", container_id, status));
    }
    let status = String::from_utf8_lossy(&status.stdout).trim().to_owned();
    if status.is_empty() {
        return Err(ObserveContainerError::InvalidState {
            container_id: container_id.to_owned(),
        });
    }
    let state = observed_state(&status);
    let endpoint = if state == ObservedRuntimeState::Running {
        Some(observe_endpoint(container_id, container_port)?)
    } else {
        None
    };

    Ok(ContainerObservation { state, endpoint })
}

fn control_container(
    operation: &'static str,
    arguments: &[&str],
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    if !is_container_id(container_id) {
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
        .filter(|endpoint| endpoint.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST))
        .ok_or_else(|| ObserveContainerError::InvalidEndpoint {
            container_id: container_id.to_owned(),
            output,
        })?;
    Ok(endpoint)
}

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

fn is_container_id(container_id: &str) -> bool {
    !container_id.is_empty() && container_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn diagnostic<'a>(stdout: &'a str, stderr: &'a str) -> &'a str {
    if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    }
}

fn container_name(application_name: &str, commit_sha: &str) -> String {
    format!("pneuma-{application_name}-{commit_sha}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_identity_is_determined_by_application_and_commit() {
        let commit_sha = "e".repeat(40);

        assert_eq!(
            container_name("personal-site", &commit_sha),
            format!("pneuma-personal-site-{commit_sha}")
        );
    }

    #[test]
    fn rejects_port_zero_before_running_podman() {
        let error = create_container("image", "personal-site", "e48c715", 0).unwrap_err();

        assert!(matches!(error, CreateContainerError::InvalidPort));
    }

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
}
