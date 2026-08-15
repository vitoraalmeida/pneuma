use std::net::SocketAddr;

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
// Captures one Podman observation and its optional loopback endpoint.
pub struct ContainerObservation {
    pub state: ObservedRuntimeState,
    pub endpoint: Option<SocketAddr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Starting,
    Running,
    Stopped,
    Failed,
    Removed,
}

#[derive(Debug, PartialEq, Eq)]
// Identifies the logical runtime materialized for a Deployment, not just its container.
pub struct RuntimeInstance {
    pub id: String,
    pub application_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
    pub state: RuntimeState,
    pub endpoint: SocketAddr,
    pub container_port: u16,
    pub observed_state: ObservedRuntimeState,
    pub observed_at: String,
    pub exit_code: Option<i32>,
    pub observation_reason: Option<String>,
    pub removed_at: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
// Supplies the identity and reserved endpoint required to register a runtime.
pub struct RuntimeRegistration {
    pub id: String,
    pub application_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
    pub endpoint: SocketAddr,
    pub container_port: u16,
}

#[derive(Debug, PartialEq, Eq)]
// Identifies the prior materialization retained during candidate replacement.
pub struct PreviousRuntime {
    pub runtime_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
}

impl RuntimeState {
    // Serializes the logical runtime lifecycle state accepted by persistence.
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Removed => "removed",
        }
    }

    // Rejects persisted logical states outside the runtime lifecycle.
    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "starting" => Some(Self::Starting),
            "running" => Some(Self::Running),
            "stopped" => Some(Self::Stopped),
            "failed" => Some(Self::Failed),
            "removed" => Some(Self::Removed),
            _ => None,
        }
    }
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
