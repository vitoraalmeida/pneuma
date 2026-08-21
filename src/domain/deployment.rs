use crate::domain::exposure::{DomainName, Visibility};
use crate::domain::git::CommitSha;
use crate::domain::identity::{ApplicationId, DeploymentId, ReleaseId, RuntimeInstanceId};
use crate::domain::release::Release;
use crate::domain::runtime::{
    ExpectedRuntimeEndpoint, ObservedRuntimeState, RuntimeRetirement, RuntimeState,
};
use std::error::Error;
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
// Records one immutable attempt to activate a Release for an Application.
pub struct Deployment {
    pub id: DeploymentId,
    pub application_id: ApplicationId,
    pub release_id: ReleaseId,
    pub deployment_type: DeploymentType,
    pub lifecycle: DeploymentLifecycle,
    pub source_revision: Option<SourceRevision>,
    pub requested_at: String,
    pub started_at: Option<String>,
}

impl Deployment {
    pub fn status(&self) -> DeploymentStatus {
        self.lifecycle.status()
    }

    pub fn is_nonterminal(&self) -> bool {
        self.lifecycle.is_nonterminal()
    }
}

#[derive(Debug, PartialEq, Eq)]
// Separates incomplete activation work from terminal results and their durable evidence.
pub enum DeploymentLifecycle {
    Pending,
    Starting,
    Verifying,
    Activating,
    Succeeded { finished_at: String },
    Failed { evidence: DeploymentFailureEvidence },
}

impl DeploymentLifecycle {
    pub fn status(&self) -> DeploymentStatus {
        match self {
            Self::Pending => DeploymentStatus::Pending,
            Self::Starting => DeploymentStatus::Starting,
            Self::Verifying => DeploymentStatus::Verifying,
            Self::Activating => DeploymentStatus::Activating,
            Self::Succeeded { .. } => DeploymentStatus::Succeeded,
            Self::Failed { .. } => DeploymentStatus::Failed,
        }
    }

    pub fn is_nonterminal(&self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Starting | Self::Verifying | Self::Activating
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
// Preserves historical failed rows that were persisted before complete diagnostics existed.
pub enum DeploymentFailureEvidence {
    Complete(DeploymentFailure),
    Incomplete,
}

#[derive(Debug, PartialEq, Eq)]
// Records complete terminal evidence for a newly written failed Deployment.
pub struct DeploymentFailure {
    pub code: String,
    pub stage: DeploymentStatus,
    pub message: String,
    pub finished_at: String,
}

impl DeploymentFailure {
    pub fn validate_details(
        code: &str,
        stage: DeploymentStatus,
        message: &str,
    ) -> Result<(), InvalidDeploymentFailure> {
        if code.is_empty()
            || code.trim() != code
            || message.is_empty()
            || message.trim() != message
            || !stage.is_nonterminal()
        {
            return Err(InvalidDeploymentFailure);
        }
        Ok(())
    }

    pub fn new(
        code: &str,
        stage: DeploymentStatus,
        message: &str,
        finished_at: String,
    ) -> Result<Self, InvalidDeploymentFailure> {
        Self::validate_details(code, stage, message)?;
        if finished_at.is_empty() {
            return Err(InvalidDeploymentFailure);
        }
        Ok(Self {
            code: code.to_owned(),
            stage,
            message: message.to_owned(),
            finished_at,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidDeploymentFailure;
impl fmt::Display for InvalidDeploymentFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("deployment failure requires trimmed code, message, timestamp, and a non-terminal stage")
    }
}
impl Error for InvalidDeploymentFailure {}

#[derive(Debug, PartialEq, Eq)]
// Couples a hydrated deployment with its immutable artifact and active marker for history views.
pub struct DeploymentHistory {
    pub deployment: Deployment,
    pub release: Release,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Preserves readable historical revisions while requiring new revisions to be full commit SHAs.
pub enum SourceRevision {
    Commit(CommitSha),
    Legacy(String),
}

impl SourceRevision {
    pub fn from_commit(commit: CommitSha) -> Self {
        Self::Commit(commit)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Commit(value) => value.as_str(),
            Self::Legacy(value) => value,
        }
    }
}
impl fmt::Display for SourceRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentType {
    Deploy,
    Rollback,
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

impl fmt::Display for DeploymentStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => formatter.write_str("pending"),
            Self::Starting => formatter.write_str("starting"),
            Self::Verifying => formatter.write_str("verifying"),
            Self::Activating => formatter.write_str("activating"),
            Self::Succeeded => formatter.write_str("succeeded"),
            Self::Failed => formatter.write_str("failed"),
        }
    }
}

impl DeploymentStatus {
    pub fn is_nonterminal(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Starting | Self::Verifying | Self::Activating
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Drives the compare-and-set state advance from one non-terminal Deployment status to the next.
pub enum DeploymentTransition {
    Start,
    RuntimeRunning,
    Verified,
}

impl DeploymentTransition {
    pub fn edge(self) -> (DeploymentStatus, DeploymentStatus) {
        match self {
            Self::Start => (DeploymentStatus::Pending, DeploymentStatus::Starting),
            Self::RuntimeRunning => (DeploymentStatus::Starting, DeploymentStatus::Verifying),
            Self::Verified => (DeploymentStatus::Verifying, DeploymentStatus::Activating),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
// Identifies a candidate whose runtime, route, and deployment were atomically promoted.
pub struct PromotedCandidate {
    pub runtime_id: RuntimeInstanceId,
    pub deployment_id: DeploymentId,
    pub finished_at: String,
}

#[derive(Debug, PartialEq, Eq)]
// Explains why a runtime cannot be promoted without changing state.
pub enum PromotionCandidateRejection {
    NotStarting { actual: RuntimeState },
    NotRunning { actual: ObservedRuntimeState },
    Removed,
}

#[derive(Debug, PartialEq, Eq)]
// Combines the persisted facts a promotion must validate before changing state.
pub struct PromotionTarget {
    pub runtime_id: RuntimeInstanceId,
    pub application_id: ApplicationId,
    pub deployment_id: DeploymentId,
    pub endpoint: ExpectedRuntimeEndpoint,
    pub state: RuntimeState,
    pub observed_state: ObservedRuntimeState,
    pub retirement: Option<RuntimeRetirement>,
    pub deployment_status: DeploymentStatus,
    pub deployment_finished_at: Option<String>,
    pub visibility: Visibility,
    pub domain: Option<DomainName>,
}

impl PromotionTarget {
    // Returns the already-confirmed promotion when the deployment has reached a terminal success.
    pub fn completed_promotion(&self) -> Option<PromotedCandidate> {
        if self.state != RuntimeState::Running
            || self.deployment_status != DeploymentStatus::Succeeded
        {
            return None;
        }
        self.deployment_finished_at
            .as_ref()
            .map(|finished_at| PromotedCandidate {
                runtime_id: self.runtime_id.clone(),
                deployment_id: self.deployment_id.clone(),
                finished_at: finished_at.clone(),
            })
    }

    // Rejects candidates that are not observed running or have been removed.
    pub fn validate_promotion_candidate(&self) -> Result<(), PromotionCandidateRejection> {
        if self.state != RuntimeState::Starting {
            return Err(PromotionCandidateRejection::NotStarting {
                actual: self.state,
            });
        }
        if self.observed_state != ObservedRuntimeState::Running {
            return Err(PromotionCandidateRejection::NotRunning {
                actual: self.observed_state.clone(),
            });
        }
        if self.retirement.is_some() {
            return Err(PromotionCandidateRejection::Removed);
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
// Selects the most recent succeeded deployment that is no longer active for rollback.
pub struct RollbackTarget {
    pub release: Release,
    pub source_revision: Option<SourceRevision>,
}
