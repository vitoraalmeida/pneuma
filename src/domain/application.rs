use std::error::Error;
use std::fmt;

use crate::domain::exposure::Visibility;
use crate::domain::identity::{ApplicationId, DeploymentId, SystemId};

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
    pub fn new(value: &str) -> Result<Self, InvalidApplicationName> {
        if !is_valid_catalog_name(value) {
            return Err(InvalidApplicationName {
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

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidApplicationName {
    pub value: String,
}
impl fmt::Display for InvalidApplicationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid catalog name `{}`", self.value)
    }
}
impl Error for InvalidApplicationName {}

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
pub enum DesiredRuntimeState {
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Collects the application settings needed to activate a deployment.
pub struct ApplicationDeploymentSpecification {
    pub application_id: ApplicationId,
    pub application_name: ApplicationName,
    pub runtime: crate::domain::runtime::RuntimeSpecification,
    pub visibility: Visibility,
}
