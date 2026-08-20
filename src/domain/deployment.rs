use crate::domain::identity::{ApplicationId, DeploymentId, ReleaseId};
use std::error::Error;
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
// Records one immutable attempt to activate a Release for an Application.
pub struct Deployment {
    pub id: DeploymentId,
    pub application_id: ApplicationId,
    pub release_id: ReleaseId,
    pub deployment_type: DeploymentType,
    pub status: DeploymentStatus,
    pub source_revision: Option<SourceRevision>,
    pub requested_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Preserves readable historical revisions while requiring new revisions to be full commit SHAs.
pub enum SourceRevision {
    CommitSha(String),
    Legacy(String),
}

impl SourceRevision {
    pub fn new(value: &str) -> Result<Self, InvalidSourceRevision> {
        if value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            Ok(Self::CommitSha(value.to_owned()))
        } else {
            Err(InvalidSourceRevision {
                value: value.to_owned(),
            })
        }
    }
    pub(crate) fn from_persisted(value: &str) -> Result<Self, InvalidSourceRevision> {
        match Self::new(value) {
            Ok(revision) => Ok(revision),
            Err(_)
                if !value.is_empty()
                    && value.trim() == value
                    && !value.chars().any(char::is_control) =>
            {
                Ok(Self::Legacy(value.to_owned()))
            }
            Err(error) => Err(error),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::CommitSha(value) | Self::Legacy(value) => value,
        }
    }
}
impl fmt::Display for SourceRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidSourceRevision {
    pub value: String,
}
impl fmt::Display for InvalidSourceRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid source revision `{}`", self.value)
    }
}
impl Error for InvalidSourceRevision {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentType {
    Deploy,
    Rollback,
}

impl DeploymentType {
    // Serializes the closed deployment origin set accepted by persistence.
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Rollback => "rollback",
        }
    }

    // Rejects persisted deployment origins outside the known domain set.
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
    // Serializes the lifecycle state recorded for an activation attempt.
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

    // Rejects persisted lifecycle states outside the deployment state machine.
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
