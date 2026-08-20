use std::error::Error;
use std::fmt;
use std::path::{Component, Path};

use crate::domain::exposure::Visibility;
use crate::domain::identity::{ApplicationId, DeploymentId, SystemId};
use crate::domain::runtime::DesiredRuntimeState;

#[derive(Debug, Clone, PartialEq, Eq)]
// Captures durable application identity and persisted runtime intent.
pub struct Application {
    pub id: ApplicationId,
    pub system_id: Option<SystemId>,
    pub name: ApplicationName,
    pub desired_runtime_state: DesiredRuntimeState,
    pub active_deployment_id: Option<DeploymentId>,
    pub specification_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Provides catalog fields without requiring callers to load the full specification.
pub struct ApplicationSummary {
    pub id: ApplicationId,
    pub system_id: Option<SystemId>,
    pub name: ApplicationName,
    pub repository: Option<String>,
    pub default_branch: Option<String>,
    pub desired_runtime_state: DesiredRuntimeState,
    pub active_deployment_id: Option<DeploymentId>,
    pub specification_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationName(String);

impl ApplicationName {
    pub fn new(value: &str) -> Result<Self, InvalidCatalogName> {
        if !is_valid_catalog_name(value) {
            return Err(InvalidCatalogName {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApplicationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemName(String);

impl SystemName {
    pub fn new(value: &str) -> Result<Self, InvalidCatalogName> {
        if !is_valid_catalog_name(value) {
            return Err(InvalidCatalogName {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SystemName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidCatalogName {
    pub value: String,
}
impl fmt::Display for InvalidCatalogName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid catalog name `{}`", self.value)
    }
}
impl Error for InvalidCatalogName {}

fn is_valid_catalog_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryKind {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Couples the persisted kind with the only location form valid for that kind.
pub enum ApplicationSource {
    Local {
        repository_path: String,
        default_branch: Option<String>,
        manifest_path: RelativeManifestPath,
    },
    Remote {
        repository_url: String,
        default_branch: Option<String>,
        manifest_path: RelativeManifestPath,
    },
}

impl ApplicationSource {
    pub fn new(
        kind: RepositoryKind,
        location: &str,
        default_branch: Option<String>,
        manifest_path: RelativeManifestPath,
    ) -> Result<Self, InvalidApplicationSource> {
        if location.is_empty()
            || location.trim() != location
            || matches!(kind, RepositoryKind::Remote) != is_remote_location(location)
        {
            return Err(InvalidApplicationSource);
        }
        Ok(match kind {
            RepositoryKind::Local => Self::Local {
                repository_path: location.to_owned(),
                default_branch,
                manifest_path,
            },
            RepositoryKind::Remote => Self::Remote {
                repository_url: location.to_owned(),
                default_branch,
                manifest_path,
            },
        })
    }
    pub fn repository_kind(&self) -> RepositoryKind {
        match self {
            Self::Local { .. } => RepositoryKind::Local,
            Self::Remote { .. } => RepositoryKind::Remote,
        }
    }
    pub fn repository_location(&self) -> &str {
        match self {
            Self::Local {
                repository_path, ..
            } => repository_path,
            Self::Remote { repository_url, .. } => repository_url,
        }
    }
    pub fn default_branch(&self) -> Option<&str> {
        match self {
            Self::Local { default_branch, .. } | Self::Remote { default_branch, .. } => {
                default_branch.as_deref()
            }
        }
    }
    pub fn manifest_path(&self) -> &RelativeManifestPath {
        match self {
            Self::Local { manifest_path, .. } | Self::Remote { manifest_path, .. } => manifest_path,
        }
    }
}

fn is_remote_location(location: &str) -> bool {
    location.contains("://") || location.starts_with("git@")
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidApplicationSource;
impl fmt::Display for InvalidApplicationSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid application source")
    }
}
impl Error for InvalidApplicationSource {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativeManifestPath(String);
impl RelativeManifestPath {
    pub fn new(value: &str) -> Result<Self, InvalidRelativeManifestPath> {
        let path = Path::new(value);
        if value.is_empty()
            || path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(InvalidRelativeManifestPath {
                path: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidRelativeManifestPath {
    pub path: String,
}
impl fmt::Display for InvalidRelativeManifestPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid relative manifest path `{}`", self.path)
    }
}
impl Error for InvalidRelativeManifestPath {}

#[derive(Debug, Clone, PartialEq, Eq)]
// Groups the HTTP response contract used to verify a runtime.
pub struct HealthCheckSpecification {
    path: HealthCheckPath,
    expected_status: HealthCheckStatus,
}
impl HealthCheckSpecification {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerPort(u16);
impl ContainerPort {
    pub fn new(value: u16) -> Result<Self, InvalidContainerPort> {
        if value == 0 {
            Err(InvalidContainerPort { value })
        } else {
            Ok(Self(value))
        }
    }
    pub fn get(self) -> u16 {
        self.0
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidContainerPort {
    pub value: u16,
}
impl fmt::Display for InvalidContainerPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid container port {}", self.value)
    }
}
impl Error for InvalidContainerPort {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheckPath(String);
impl HealthCheckPath {
    pub fn new(value: &str) -> Result<Self, InvalidHealthCheckPath> {
        if !value.starts_with('/') || value.chars().any(char::is_whitespace) {
            Err(InvalidHealthCheckPath {
                value: value.to_owned(),
            })
        } else {
            Ok(Self(value.to_owned()))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidHealthCheckPath {
    pub value: String,
}
impl fmt::Display for InvalidHealthCheckPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid health check path `{}`", self.value)
    }
}
impl Error for InvalidHealthCheckPath {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthCheckStatus(u16);
impl HealthCheckStatus {
    pub fn new(value: u16) -> Result<Self, InvalidHealthCheckStatus> {
        if !(100..=599).contains(&value) {
            Err(InvalidHealthCheckStatus { value })
        } else {
            Ok(Self(value))
        }
    }
    pub fn get(self) -> u16 {
        self.0
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidHealthCheckStatus {
    pub value: u16,
}
impl fmt::Display for InvalidHealthCheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid health check status {}", self.value)
    }
}
impl Error for InvalidHealthCheckStatus {}

#[derive(Debug, Clone, PartialEq, Eq)]
// Collects the application settings needed to activate a deployment.
pub struct ApplicationDeploymentSpecification {
    pub application_id: ApplicationId,
    pub application_name: ApplicationName,
    pub runtime: RuntimeSpecification,
    pub visibility: Visibility,
}
