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
pub struct RuntimeRegistration {
    pub id: String,
    pub application_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
    pub endpoint: SocketAddr,
    pub container_port: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PreviousRuntime {
    pub runtime_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
}

impl RuntimeState {
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Removed => "removed",
        }
    }

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
    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "running" => Some(Self::Running),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }

    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
        }
    }
}
