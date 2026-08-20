use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::domain::identity::{ApplicationId, ContainerId, DeploymentId, RuntimeInstanceId};

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

impl ObservedRuntimeState {
    // Preserves an adapter-reported state string, including unknown future values.
    pub fn database_value(&self) -> &str {
        match self {
            Self::Missing => "missing",
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Unknown { status } => status,
        }
    }

    // Maps persisted observations while retaining unrecognized adapter status text.
    pub fn from_database(value: &str) -> Self {
        match value {
            "missing" => Self::Missing,
            "created" => Self::Created,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "stopping" => Self::Stopping,
            "stopped" => Self::Stopped,
            "failed" => Self::Failed,
            status => Self::Unknown {
                status: status.to_owned(),
            },
        }
    }

    // Uses a stable marker for unknown observations while retaining their diagnostic text in memory.
    pub(crate) fn persisted_value(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Unknown { .. } => "unknown",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeEndpointError {
    NotIpv4Loopback { endpoint: SocketAddr },
}

impl std::fmt::Display for RuntimeEndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotIpv4Loopback { endpoint } => write!(
                formatter,
                "runtime endpoint must be IPv4 loopback with a nonzero port: {endpoint}"
            ),
        }
    }
}

impl std::error::Error for RuntimeEndpointError {}

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
}

#[derive(Debug, PartialEq, Eq)]
// Captures one Podman observation. Only a confirmed running container carries an endpoint.
pub enum ContainerObservation {
    Running { observed_endpoint: SocketAddr },
    NotRunning { state: ObservedRuntimeState },
}

impl ContainerObservation {
    pub fn running(observed_endpoint: SocketAddr) -> Result<Self, RuntimeEndpointError> {
        validate_loopback_endpoint(observed_endpoint)?;
        Ok(Self::Running { observed_endpoint })
    }

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

    pub fn state(&self) -> &ObservedRuntimeState {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Starting,
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
// Records explicit retirement evidence; absence means the runtime remains logically active.
pub struct RuntimeRetirement {
    pub removed_at: String,
}

#[derive(Debug, PartialEq, Eq)]
// Identifies the logical runtime materialized for a Deployment, not just its container.
pub struct RuntimeInstance {
    pub id: RuntimeInstanceId,
    pub application_id: ApplicationId,
    pub deployment_id: DeploymentId,
    pub external_runtime_id: ContainerId,
    pub state: RuntimeState,
    pub expected_endpoint: ExpectedRuntimeEndpoint,
    pub container_port: u16,
    pub observed_state: ObservedRuntimeState,
    pub observed_at: String,
    pub exit_code: Option<i32>,
    pub observation_reason: Option<String>,
    pub retirement: Option<RuntimeRetirement>,
}

#[derive(Debug, PartialEq, Eq)]
// Supplies the identity and reserved endpoint required to register a runtime.
pub struct RuntimeRegistration {
    pub id: RuntimeInstanceId,
    pub application_id: ApplicationId,
    pub deployment_id: DeploymentId,
    pub external_runtime_id: ContainerId,
    pub expected_endpoint: ExpectedRuntimeEndpoint,
    pub container_port: u16,
}

#[derive(Debug, PartialEq, Eq)]
// Identifies the prior materialization retained during candidate replacement.
pub struct PreviousRuntime {
    pub runtime_id: RuntimeInstanceId,
    pub deployment_id: DeploymentId,
    pub external_runtime_id: ContainerId,
}

impl RuntimeState {
    // Serializes the logical runtime lifecycle state accepted by persistence.
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    // Rejects persisted logical states outside the runtime lifecycle.
    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "starting" => Some(Self::Starting),
            "running" => Some(Self::Running),
            "stopped" => Some(Self::Stopped),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

fn validate_loopback_endpoint(endpoint: SocketAddr) -> Result<(), RuntimeEndpointError> {
    if endpoint.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || endpoint.port() == 0 {
        return Err(RuntimeEndpointError::NotIpv4Loopback { endpoint });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesiredRuntimeState {
    Running,
    Stopped,
}

impl DesiredRuntimeState {
    // Rejects persisted runtime intent outside the operator-controlled choices.
    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "running" => Some(Self::Running),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }

    // Serializes the operator's requested runtime intent.
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
        }
    }
}
