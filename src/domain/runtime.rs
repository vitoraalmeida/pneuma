use std::fmt;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use thiserror::Error;

use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
// Preserves adapter-provided container text separately from logical runtime identity.
pub struct ContainerId(String);

impl ContainerId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    // Rejects empty or non-hexadecimal external container text before it reaches Podman or SQLite.
    pub(crate) fn is_valid(value: &str) -> bool {
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
}

impl From<String> for ContainerId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ContainerId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for ContainerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

// External state as last reported by the container authority. `Unknown`
// preserves unrecognized status text verbatim instead of collapsing it into a
// known state, so observation never invents facts (the reconciliation policy
// treats unknown states conservatively).
#[derive(Clone, Debug, PartialEq, Eq)]
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

impl std::fmt::Display for ObservedRuntimeState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("missing"),
            Self::Created => formatter.write_str("created"),
            Self::Starting => formatter.write_str("starting"),
            Self::Running => formatter.write_str("running"),
            Self::Stopping => formatter.write_str("stopping"),
            Self::Stopped => formatter.write_str("stopped"),
            Self::Failed => formatter.write_str("failed"),
            Self::Unknown { status } => formatter.write_str(status),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum RuntimeEndpointError {
    #[error("runtime endpoint must be IPv4 loopback with a nonzero port: {endpoint}")]
    NotIpv4Loopback { endpoint: SocketAddr },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Identifies the loopback endpoint reserved for a logical runtime before external effects.
pub struct ExpectedRuntimeEndpoint(SocketAddr);

impl ExpectedRuntimeEndpoint {
    pub fn new(endpoint: SocketAddr) -> Result<Self, RuntimeEndpointError> {
        validate_loopback_endpoint(endpoint)?;
        Ok(Self(endpoint))
    }

    pub fn socket_addr(self) -> SocketAddr {
        self.0
    }

    // Returns the published loopback port; loopback validation already rejected zero.
    pub(crate) fn host_port(&self) -> Result<HostPort, InvalidHostPort> {
        HostPort::new(self.0.port())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Captures one Podman observation. Only a confirmed running container carries an endpoint.
pub enum ContainerObservation {
    Running { observed_endpoint: SocketAddr },
    NotRunning { state: ObservedRuntimeState },
}

impl ContainerObservation {
    // A running observation must re-prove the loopback endpoint rule: adapters
    // may only report endpoints that satisfy the same invariant as expected ones.
    pub fn running(observed_endpoint: SocketAddr) -> Result<Self, RuntimeEndpointError> {
        validate_loopback_endpoint(observed_endpoint)?;
        Ok(Self::Running { observed_endpoint })
    }

    // Rejects `Running` here so callers cannot construct a contradictory
    // "not running but running" observation.
    pub fn not_running(state: ObservedRuntimeState) -> Result<Self, ObservedRuntimeState> {
        if state == ObservedRuntimeState::Running {
            return Err(state);
        }
        Ok(Self::NotRunning { state })
    }

    pub fn missing() -> Self {
        Self::NotRunning {
            state: ObservedRuntimeState::Missing,
        }
    }

    pub(crate) fn state(&self) -> &ObservedRuntimeState {
        match self {
            Self::Running { .. } => &ObservedRuntimeState::Running,
            Self::NotRunning { state } => state,
        }
    }

    pub fn observed_endpoint(&self) -> Option<SocketAddr> {
        match self {
            Self::Running { observed_endpoint } => Some(*observed_endpoint),
            Self::NotRunning { .. } => None,
        }
    }
}

// Pneuma's own lifecycle record for a logical runtime (`starting` while the
// candidate runs pre-promotion checks). Deliberately distinct from
// `ObservedRuntimeState`: recorded state is what Pneuma believes; observed
// state is what Podman reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Starting,
    Running,
    Stopped,
    Failed,
}

impl std::fmt::Display for RuntimeState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => formatter.write_str("starting"),
            Self::Running => formatter.write_str("running"),
            Self::Stopped => formatter.write_str("stopped"),
            Self::Failed => formatter.write_str("failed"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Records explicit retirement evidence; absence means the runtime remains logically active.
pub struct RuntimeRetirement {
    pub removed_at: String,
}

#[derive(Debug, PartialEq, Eq)]
// Entity: the logical runtime materialized for a Deployment — the invariant
// authority for runtime identity, endpoint, and retirement facts, not just its
// container. The `observed_*` fields are the last external observation snapshot;
// `retirement` records intentional removal so reconciliation can tell
// tombstones from drift.
pub struct RuntimeInstance {
    pub id: RuntimeInstanceId,
    pub application_id: ApplicationId,
    pub deployment_id: DeploymentId,
    pub external_runtime_id: ContainerId,
    pub state: RuntimeState,
    pub expected_endpoint: ExpectedRuntimeEndpoint,
    pub container_port: ContainerPort,
    pub observed_state: ObservedRuntimeState,
    pub observed_at: String,
    pub exit_code: Option<i32>,
    pub observation_reason: Option<String>,
    pub retirement: Option<RuntimeRetirement>,
}

#[derive(Debug, PartialEq, Eq)]
// Supplies the identity and reserved endpoint required to register a runtime.
pub(crate) struct RuntimeRegistration {
    pub(crate) id: RuntimeInstanceId,
    pub(crate) application_id: ApplicationId,
    pub(crate) deployment_id: DeploymentId,
    pub(crate) external_runtime_id: ContainerId,
    pub(crate) expected_endpoint: ExpectedRuntimeEndpoint,
    pub(crate) container_port: ContainerPort,
}

#[derive(Debug, PartialEq, Eq)]
// Identifies the prior materialization retained during candidate replacement.
pub(crate) struct PreviousRuntime {
    pub(crate) runtime_id: RuntimeInstanceId,
    pub(crate) deployment_id: DeploymentId,
    pub(crate) external_runtime_id: ContainerId,
}

// Port inside the container the application listens on (from the manifest).
// Distinct from `HostPort` so a published loopback port can never be confused
// with the container-facing port at type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerPort(u16);

impl ContainerPort {
    pub fn new(value: u16) -> Result<Self, InvalidContainerPort> {
        if value == 0 {
            return Err(InvalidContainerPort { value });
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid container port {value}")]
pub struct InvalidContainerPort {
    pub value: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Identifies the loopback host port published by a runtime container.
pub struct HostPort(u16);

impl HostPort {
    pub fn new(value: u16) -> Result<Self, InvalidHostPort> {
        if value == 0 {
            return Err(InvalidHostPort { value });
        }
        Ok(Self(value))
    }

    pub(crate) fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid host port {value}")]
pub struct InvalidHostPort {
    pub value: u16,
}

// HTTP path probed to verify runtime health. Must start with `/` and stay
// whitespace-free so it can be embedded safely in curl invocations and
// rendered configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheckPath(String);

impl HealthCheckPath {
    pub fn new(value: &str) -> Result<Self, InvalidHealthCheckPath> {
        if !value.starts_with('/') || value.chars().any(char::is_whitespace) {
            return Err(InvalidHealthCheckPath {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid health check path `{value}`")]
pub struct InvalidHealthCheckPath {
    pub value: String,
}

// HTTP status considered healthy, bounded to the valid HTTP range so adapters
// never compare against an impossible expectation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthCheckStatus(u16);

impl HealthCheckStatus {
    pub fn new(value: u16) -> Result<Self, InvalidHealthCheckStatus> {
        if !(100..=599).contains(&value) {
            return Err(InvalidHealthCheckStatus { value });
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid health check status {value}")]
pub struct InvalidHealthCheckStatus {
    pub value: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Groups the HTTP response contract used to verify a runtime.
pub struct HealthCheckSpecification {
    path: HealthCheckPath,
    expected_status: HealthCheckStatus,
}

impl HealthCheckSpecification {
    // Bundles already-validated parts; there is no way to build an unhealthy
    // combination because each field is a validated value object.
    pub fn new(path: HealthCheckPath, expected_status: HealthCheckStatus) -> Self {
        Self {
            path,
            expected_status,
        }
    }

    pub fn path(&self) -> &HealthCheckPath {
        &self.path
    }

    pub fn expected_status(&self) -> HealthCheckStatus {
        self.expected_status
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Defines the container endpoint and health contract from the specification.
pub struct RuntimeSpecification {
    container_port: ContainerPort,
    health_check: HealthCheckSpecification,
}

impl RuntimeSpecification {
    pub fn new(container_port: ContainerPort, health_check: HealthCheckSpecification) -> Self {
        Self {
            container_port,
            health_check,
        }
    }

    pub fn container_port(&self) -> ContainerPort {
        self.container_port
    }

    pub fn health_check(&self) -> &HealthCheckSpecification {
        &self.health_check
    }
}

// Single owner of the loopback endpoint invariant: IPv4 127.0.0.1 with a
// nonzero port. Both expected endpoints and observed running endpoints must
// pass through here, which is what closed the historical `::1` drift.
pub(crate) fn validate_loopback_endpoint(endpoint: SocketAddr) -> Result<(), RuntimeEndpointError> {
    if endpoint.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || endpoint.port() == 0 {
        return Err(RuntimeEndpointError::NotIpv4Loopback { endpoint });
    }
    Ok(())
}

// Derives the one stable external name shared by the Quadlet unit and the Podman container so
// supervision, observation, and reconciliation always address the same runtime identity.
pub(crate) fn stable_runtime_name(application_name: &str, deployment_id: &str) -> String {
    format!("pneuma-{application_name}-{deployment_id}")
}

#[cfg(test)]
mod tests {
    use super::{ContainerId, ExpectedRuntimeEndpoint, stable_runtime_name};
    use crate::domain::identity::{ApplicationId, DeploymentId};

    #[test]
    fn logical_and_external_id_apis_are_not_interchangeable() {
        fn deployment_for(_application_id: ApplicationId, _deployment_id: DeploymentId) {}
        fn observe_container(_container_id: ContainerId) {}

        deployment_for(
            ApplicationId::new("11111111111111111111111111111111").unwrap(),
            DeploymentId::new("22222222222222222222222222222222").unwrap(),
        );
        observe_container(ContainerId::from("container"));
    }

    #[test]
    fn container_identity_text_must_be_nonempty_hexadecimal() {
        assert!(ContainerId::is_valid("0123abcdefABCDEF"));
        assert!(!ContainerId::is_valid(""));
        assert!(!ContainerId::is_valid("container id"));
        assert!(!ContainerId::is_valid("0123 abcdef"));
    }

    #[test]
    fn expected_endpoints_are_ipv4_loopback_with_a_port() {
        assert!(ExpectedRuntimeEndpoint::new("127.0.0.1:30000".parse().unwrap()).is_ok());
        for invalid in [
            "10.0.0.5:30000",
            "0.0.0.0:30000",
            "[::1]:30000",
            "127.0.0.1:0",
        ] {
            assert!(
                ExpectedRuntimeEndpoint::new(invalid.parse().unwrap()).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn stable_names_couple_the_application_and_deployment_identities() {
        assert_eq!(
            stable_runtime_name(
                ApplicationId::new("11111111111111111111111111111111")
                    .unwrap()
                    .as_str(),
                DeploymentId::new("22222222222222222222222222222222")
                    .unwrap()
                    .as_str()
            ),
            "pneuma-11111111111111111111111111111111-22222222222222222222222222222222"
        );
    }
}
