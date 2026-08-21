use std::error::Error;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::Command;

use crate::domain::reconciliation::NamedContainerObservation;
use crate::domain::runtime::ContainerId;
use crate::domain::runtime::{ContainerObservation, ObservedRuntimeState};

#[derive(Debug, PartialEq, Eq)]
// Preserves Podman diagnostics from successful lifecycle commands for callers that report effects.
pub struct ContainerCommandOutput {
    pub stdout: String,
    pub stderr: String,
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

#[derive(Debug)]
pub enum ResolveContainerError {
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
pub fn start_container(
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("starting", &["start"], container_id)
}

// Resolves Podman's current container ID by stable name because recreation changes external IDs.
pub fn resolve_container_id(name: &str) -> Result<String, ResolveContainerError> {
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
    if !is_container_id(id) {
        return Err(ResolveContainerError::InvalidOutput {
            name: name.to_owned(),
        });
    }
    Ok(id.to_owned())
}

// Observes a deterministic container name without treating ordinary absence as an adapter failure.
pub fn observe_named_container(
    name: &str,
    container_port: u16,
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
    if !is_container_id(id) || observed_name.is_empty() || image_reference.is_empty() {
        return Err(ObserveNamedContainerError::InvalidOutput {
            name: name.to_owned(),
        });
    }
    let observation = observe_container(id, container_port)
        .map_err(|source| ObserveNamedContainerError::Observe { source })?;
    Ok(NamedContainerObservation::Present {
        id: ContainerId::from(id),
        name: observed_name.to_owned(),
        image_reference: image_reference.to_owned(),
        application_label: (!application_label.is_empty()).then(|| application_label.to_owned()),
        image_digest_label: (!image_digest_label.is_empty()).then(|| image_digest_label.to_owned()),
        observation,
    })
}

// Stops a validated container through the shared Podman lifecycle command path.
pub fn stop_container(container_id: &str) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("stopping", &["stop"], container_id)
}

// Force-removes a validated candidate container during cleanup after a failed deployment.
pub fn remove_container(
    container_id: &str,
) -> Result<ContainerCommandOutput, ControlContainerError> {
    control_container("removing", &["container", "rm", "--force"], container_id)
}

// Observes container state and exposes an endpoint only while Podman confirms it is running.
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
        return Ok(ContainerObservation::missing());
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
    if state == ObservedRuntimeState::Running {
        ContainerObservation::running(observe_endpoint(container_id, container_port)?).map_err(
            |_| ObserveContainerError::InvalidEndpoint {
                container_id: container_id.to_owned(),
                output: "Podman returned a non-loopback endpoint".to_owned(),
            },
        )
    } else {
        ContainerObservation::not_running(state).map_err(|_| ObserveContainerError::InvalidState {
            container_id: container_id.to_owned(),
        })
    }
}

// Executes a lifecycle command after validating the external container ID used as its target.
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
        .filter(|endpoint| endpoint.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST))
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

// Rejects empty or non-hexadecimal values before passing a container ID to Podman.
fn is_container_id(container_id: &str) -> bool {
    !container_id.is_empty() && container_id.bytes().all(|byte| byte.is_ascii_hexdigit())
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
}
