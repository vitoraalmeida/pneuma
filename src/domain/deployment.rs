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

// Names one external fact or workflow decision that asks a Deployment to change state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentEvent {
    Start,
    RuntimeRunning,
    Verified,
    Activated,
    Fail,
}

impl fmt::Display for DeploymentEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start => formatter.write_str("start"),
            Self::RuntimeRunning => formatter.write_str("runtime running"),
            Self::Verified => formatter.write_str("verified"),
            Self::Activated => formatter.write_str("activated"),
            Self::Fail => formatter.write_str("fail"),
        }
    }
}

impl DeploymentStatus {
    // Applies one event to the current status, returning the next status or rejecting the pair.
    pub fn transition(
        self,
        event: DeploymentEvent,
    ) -> Result<DeploymentStatus, InvalidDeploymentTransition> {
        let next = match (self, event) {
            (Self::Pending, DeploymentEvent::Start) => Self::Starting,
            (Self::Starting, DeploymentEvent::RuntimeRunning) => Self::Verifying,
            (Self::Verifying, DeploymentEvent::Verified) => Self::Activating,
            (Self::Verifying | Self::Activating, DeploymentEvent::Activated) => Self::Succeeded,
            (status, DeploymentEvent::Fail) if status.can_fail() => Self::Failed,
            _ => {
                return Err(InvalidDeploymentTransition {
                    current: self,
                    event,
                });
            }
        };
        Ok(next)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }

    pub fn is_nonterminal(self) -> bool {
        !self.is_terminal()
    }

    // Only a Deployment still performing activation work may record a terminal failure.
    pub fn can_fail(self) -> bool {
        self.is_nonterminal()
    }
}

#[derive(Debug, PartialEq, Eq)]
// Rejects an event that has no legal edge from the current Deployment status.
pub struct InvalidDeploymentTransition {
    pub current: DeploymentStatus,
    pub event: DeploymentEvent,
}

impl fmt::Display for InvalidDeploymentTransition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot apply deployment event `{}` while in state `{}`",
            self.event, self.current
        )
    }
}
impl Error for InvalidDeploymentTransition {}

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
            return Err(PromotionCandidateRejection::NotStarting { actual: self.state });
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

#[cfg(test)]
mod tests {
    use super::{DeploymentEvent, DeploymentStatus};

    const STATUSES: [DeploymentStatus; 6] = [
        DeploymentStatus::Pending,
        DeploymentStatus::Starting,
        DeploymentStatus::Verifying,
        DeploymentStatus::Activating,
        DeploymentStatus::Succeeded,
        DeploymentStatus::Failed,
    ];

    const EVENTS: [DeploymentEvent; 5] = [
        DeploymentEvent::Start,
        DeploymentEvent::RuntimeRunning,
        DeploymentEvent::Verified,
        DeploymentEvent::Activated,
        DeploymentEvent::Fail,
    ];

    #[test]
    fn applies_every_valid_transition() {
        let valid = [
            (
                DeploymentStatus::Pending,
                DeploymentEvent::Start,
                DeploymentStatus::Starting,
            ),
            (
                DeploymentStatus::Pending,
                DeploymentEvent::Fail,
                DeploymentStatus::Failed,
            ),
            (
                DeploymentStatus::Starting,
                DeploymentEvent::RuntimeRunning,
                DeploymentStatus::Verifying,
            ),
            (
                DeploymentStatus::Starting,
                DeploymentEvent::Fail,
                DeploymentStatus::Failed,
            ),
            (
                DeploymentStatus::Verifying,
                DeploymentEvent::Verified,
                DeploymentStatus::Activating,
            ),
            (
                DeploymentStatus::Verifying,
                DeploymentEvent::Activated,
                DeploymentStatus::Succeeded,
            ),
            (
                DeploymentStatus::Verifying,
                DeploymentEvent::Fail,
                DeploymentStatus::Failed,
            ),
            (
                DeploymentStatus::Activating,
                DeploymentEvent::Activated,
                DeploymentStatus::Succeeded,
            ),
            (
                DeploymentStatus::Activating,
                DeploymentEvent::Fail,
                DeploymentStatus::Failed,
            ),
        ];
        for (current, event, expected) in valid {
            assert_eq!(
                current.transition(event),
                Ok(expected),
                "{current} on {event}"
            );
        }
    }

    #[test]
    fn rejects_every_invalid_transition_with_current_and_event() {
        let valid = [
            (DeploymentStatus::Pending, DeploymentEvent::Start),
            (DeploymentStatus::Pending, DeploymentEvent::Fail),
            (DeploymentStatus::Starting, DeploymentEvent::RuntimeRunning),
            (DeploymentStatus::Starting, DeploymentEvent::Fail),
            (DeploymentStatus::Verifying, DeploymentEvent::Verified),
            (DeploymentStatus::Verifying, DeploymentEvent::Activated),
            (DeploymentStatus::Verifying, DeploymentEvent::Fail),
            (DeploymentStatus::Activating, DeploymentEvent::Activated),
            (DeploymentStatus::Activating, DeploymentEvent::Fail),
        ];
        for current in STATUSES {
            for event in EVENTS {
                if valid.contains(&(current, event)) {
                    continue;
                }
                let error = match current.transition(event) {
                    Err(error) => error,
                    Ok(next) => panic!("{current} on {event} must be rejected, got {next}"),
                };
                assert_eq!(error.current, current);
                assert_eq!(error.event, event);
            }
        }
    }

    #[test]
    fn terminal_states_report_terminal_and_cannot_fail() {
        for status in STATUSES {
            let terminal = matches!(
                status,
                DeploymentStatus::Succeeded | DeploymentStatus::Failed
            );
            assert_eq!(status.is_terminal(), terminal, "{status}");
            assert_eq!(status.is_nonterminal(), !terminal, "{status}");
            assert_eq!(status.can_fail(), !terminal, "{status}");
        }
    }

    #[test]
    fn transition_error_names_current_state_and_event() {
        let error = match DeploymentStatus::Failed.transition(DeploymentEvent::Start) {
            Err(error) => error,
            Ok(next) => panic!("failed deployments cannot restart, got {next}"),
        };
        assert_eq!(
            error.to_string(),
            "cannot apply deployment event `start` while in state `failed`"
        );
    }
}
