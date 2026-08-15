use crate::domain::exposure::Visibility;
use crate::domain::manifest::DeliveryType;
use crate::domain::runtime::DesiredRuntimeState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub id: String,
    pub system_id: Option<String>,
    pub name: String,
    pub desired_runtime_state: DesiredRuntimeState,
    pub active_deployment_id: Option<String>,
    pub specification_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationSummary {
    pub id: String,
    pub system_id: Option<String>,
    pub name: String,
    pub repository: Option<String>,
    pub default_branch: Option<String>,
    pub desired_runtime_state: DesiredRuntimeState,
    pub active_deployment_id: Option<String>,
    pub specification_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryKind {
    Local,
    Remote,
}

impl RepositoryKind {
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "remote" => Some(Self::Remote),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationSource {
    pub repository_url: String,
    pub repository_kind: RepositoryKind,
    pub default_branch: Option<String>,
    pub manifest_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverySpecification {
    pub delivery_type: DeliveryType,
    pub image_repository: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheckSpecification {
    pub path: String,
    pub expected_status: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSpecification {
    pub container_port: u16,
    pub health_check: HealthCheckSpecification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationDeploymentSpecification {
    pub application_id: String,
    pub application_name: String,
    pub runtime: RuntimeSpecification,
    pub visibility: Visibility,
}
