#[derive(Debug, PartialEq, Eq)]
pub struct Deployment {
    pub id: String,
    pub application_id: String,
    pub release_id: String,
    pub deployment_type: DeploymentType,
    pub status: DeploymentStatus,
    pub source_revision: Option<String>,
    pub requested_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentType {
    Deploy,
    Rollback,
}

impl DeploymentType {
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Rollback => "rollback",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "deploy" => Some(Self::Deploy),
            "rollback" => Some(Self::Rollback),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentStatus {
    Pending,
    Starting,
    Verifying,
    Activating,
    Succeeded,
    Failed,
}

impl DeploymentStatus {
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Starting => "starting",
            Self::Verifying => "verifying",
            Self::Activating => "activating",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "starting" => Some(Self::Starting),
            "verifying" => Some(Self::Verifying),
            "activating" => Some(Self::Activating),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}
