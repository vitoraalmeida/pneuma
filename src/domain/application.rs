use crate::domain::exposure::Visibility;
use crate::domain::identity::{ApplicationId, DeploymentId, SystemId};
use crate::domain::runtime::DesiredRuntimeState;

#[derive(Debug, Clone, PartialEq, Eq)]
// Captures durable application identity and persisted runtime intent.
pub struct Application {
    pub id: ApplicationId,
    pub system_id: Option<SystemId>,
    pub name: String,
    pub desired_runtime_state: DesiredRuntimeState,
    pub active_deployment_id: Option<DeploymentId>,
    pub specification_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Provides catalog fields without requiring callers to load the full specification.
pub struct ApplicationSummary {
    pub id: ApplicationId,
    pub system_id: Option<SystemId>,
    pub name: String,
    pub repository: Option<String>,
    pub default_branch: Option<String>,
    pub desired_runtime_state: DesiredRuntimeState,
    pub active_deployment_id: Option<DeploymentId>,
    pub specification_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryKind {
    Local,
    Remote,
}

impl RepositoryKind {
    // Serializes the closed repository-origin set accepted by persistence.
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    // Rejects persisted origin values outside the known domain set.
    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "remote" => Some(Self::Remote),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Defines the imported repository boundary and optional branch selection.
pub struct ApplicationSource {
    pub repository_url: String,
    pub repository_kind: RepositoryKind,
    pub default_branch: Option<String>,
    pub manifest_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Groups the HTTP response contract used to verify a runtime.
pub struct HealthCheckSpecification {
    pub path: String,
    pub expected_status: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Defines the container endpoint and health contract from the specification.
pub struct RuntimeSpecification {
    pub container_port: u16,
    pub health_check: HealthCheckSpecification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Collects the application settings needed to activate a deployment.
pub struct ApplicationDeploymentSpecification {
    pub application_id: ApplicationId,
    pub application_name: String,
    pub runtime: RuntimeSpecification,
    pub visibility: Visibility,
}
