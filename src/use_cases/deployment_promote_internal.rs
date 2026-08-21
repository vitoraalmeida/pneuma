use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::health_check_internal::{
    HealthCheckError, HealthCheckFailure, HealthCheckResult, check_internal_health,
};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::domain::deployment::DeploymentStatus;
use crate::domain::exposure::Visibility;
use crate::domain::identity::{DeploymentId, RuntimeInstanceId};
use crate::domain::runtime::HealthCheckSpecification;
use crate::domain::runtime::{ObservedRuntimeState, RuntimeState};
use crate::use_cases::deployment_transition::{TransitionDeploymentError, fail_deployment};

#[derive(Debug, PartialEq, Eq)]
// Identifies a candidate whose promotion was atomically confirmed.
pub struct PromotedCandidate {
    pub runtime_id: RuntimeInstanceId,
    pub deployment_id: DeploymentId,
    pub finished_at: String,
}

#[derive(Debug)]
pub enum PromoteInternalCandidateError {
    RuntimeNotFound {
        runtime_id: String,
    },
    InvalidRuntimeState {
        runtime_id: String,
        actual: String,
    },
    RuntimeNotRunning {
        runtime_id: String,
        actual: String,
    },
    RuntimeRemoved {
        runtime_id: String,
    },
    InvalidDeploymentState {
        deployment_id: String,
        actual: String,
    },
    PublicApplication {
        application_id: String,
    },
    HealthCheck {
        source: HealthCheckError,
    },
    CandidateUnhealthy {
        result: HealthCheckResult,
    },
    RecordFailure {
        source: TransitionDeploymentError,
    },
    Store {
        source: DeploymentStoreError,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for PromoteInternalCandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeNotFound { runtime_id } => {
                write!(formatter, "runtime `{runtime_id}` was not found")
            }
            Self::InvalidRuntimeState { runtime_id, actual } => write!(
                formatter,
                "runtime `{runtime_id}` must be Starting to be promoted, but is `{actual}`"
            ),
            Self::RuntimeNotRunning { runtime_id, actual } => write!(
                formatter,
                "runtime `{runtime_id}` must be Running to be promoted, but is `{actual}`"
            ),
            Self::RuntimeRemoved { runtime_id } => {
                write!(formatter, "runtime `{runtime_id}` has already been removed")
            }
            Self::InvalidDeploymentState {
                deployment_id,
                actual,
            } => write!(
                formatter,
                "deployment `{deployment_id}` must be Verifying to promote its candidate, but is `{actual}`"
            ),
            Self::PublicApplication { application_id } => write!(
                formatter,
                "application `{application_id}` requires public route activation before promotion"
            ),
            Self::HealthCheck { source } => write!(formatter, "{source}"),
            Self::CandidateUnhealthy { result } => {
                write!(
                    formatter,
                    "candidate failed its internal health check: {result:?}"
                )
            }
            Self::RecordFailure { source } => {
                write!(
                    formatter,
                    "failed to record candidate health failure: {source}"
                )
            }
            Self::Store { source } => {
                write!(formatter, "failed to promote internal candidate: {source}")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to promote internal candidate: {source}")
            }
        }
    }
}

impl Error for PromoteInternalCandidateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::HealthCheck { source } => Some(source),
            Self::RecordFailure { source } => Some(source),
            Self::Store { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::RuntimeNotFound { .. }
            | Self::InvalidRuntimeState { .. }
            | Self::RuntimeNotRunning { .. }
            | Self::RuntimeRemoved { .. }
            | Self::InvalidDeploymentState { .. }
            | Self::PublicApplication { .. }
            | Self::CandidateUnhealthy { .. } => None,
        }
    }
}

// Health-checks an internal candidate outside a transaction, then atomically promotes it.
pub fn promote_internal_candidate(
    connection: &mut Connection,
    runtime_id: &str,
    health_check: &HealthCheckSpecification,
) -> Result<PromotedCandidate, PromoteInternalCandidateError> {
    let target = load_target(connection, runtime_id)?;
    if let Some(promoted) = completed_promotion(&target) {
        return Ok(promoted);
    }
    validate_target(&target)?;

    let health = check_internal_health(
        target.endpoint,
        health_check.path().as_str(),
        health_check.expected_status().get(),
    )
    .map_err(|source| PromoteInternalCandidateError::HealthCheck { source })?;
    match health {
        HealthCheckResult::Healthy { .. } => {}
        HealthCheckResult::Unhealthy { ref failure, .. } => {
            let message = health_failure_message(failure);
            fail_deployment(
                connection,
                target.deployment_id.as_str(),
                "health_check_failed",
                &message,
            )
            .map_err(|source| PromoteInternalCandidateError::RecordFailure { source })?;
            return Err(PromoteInternalCandidateError::CandidateUnhealthy { result: health });
        }
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
    let target = load_target(&transaction, runtime_id)?;
    if let Some(promoted) = completed_promotion(&target) {
        transaction
            .commit()
            .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
        return Ok(promoted);
    }
    validate_target(&target)?;

    let updated = deployment_store::promote_internal(&transaction, &target)
        .map_err(|source| PromoteInternalCandidateError::Store { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: "changed during promotion".to_owned(),
        });
    }
    let finished_at =
        deployment_store::load_finished_at(&transaction, target.deployment_id.as_str())
            .map_err(|source| PromoteInternalCandidateError::Store { source })?;
    transaction
        .commit()
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;

    Ok(PromotedCandidate {
        runtime_id: target.runtime_id,
        deployment_id: target.deployment_id,
        finished_at,
    })
}

// Captures the persisted facts that must remain valid throughout promotion.
type PromotionTarget = deployment_store::PromotionTarget;

// Loads and validates persisted state text before making promotion decisions.
fn load_target(
    connection: &Connection,
    runtime_id: &str,
) -> Result<PromotionTarget, PromoteInternalCandidateError> {
    deployment_store::load_promotion_target(connection, runtime_id)
        .map_err(|source| PromoteInternalCandidateError::Store { source })?
        .ok_or_else(|| PromoteInternalCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_owned(),
        })
}

// Recognizes an already committed promotion so retries remain idempotent.
fn completed_promotion(target: &PromotionTarget) -> Option<PromotedCandidate> {
    if target.state != RuntimeState::Running
        || target.deployment_status != DeploymentStatus::Succeeded
    {
        return None;
    }
    target
        .deployment_finished_at
        .as_ref()
        .map(|finished_at| PromotedCandidate {
            runtime_id: target.runtime_id.clone(),
            deployment_id: target.deployment_id.clone(),
            finished_at: finished_at.clone(),
        })
}

// Prevents an unobserved, removed, or public candidate from bypassing route activation.
fn validate_target(target: &PromotionTarget) -> Result<(), PromoteInternalCandidateError> {
    if target.state != RuntimeState::Starting {
        return Err(PromoteInternalCandidateError::InvalidRuntimeState {
            runtime_id: target.runtime_id.to_string(),
            actual: target.state.to_string(),
        });
    }
    if target.observed_state != ObservedRuntimeState::Running {
        return Err(PromoteInternalCandidateError::RuntimeNotRunning {
            runtime_id: target.runtime_id.to_string(),
            actual: target.observed_state.to_string(),
        });
    }
    if target.retirement.is_some() {
        return Err(PromoteInternalCandidateError::RuntimeRemoved {
            runtime_id: target.runtime_id.to_string(),
        });
    }
    if target.deployment_status != DeploymentStatus::Verifying {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: target.deployment_status.to_string(),
        });
    }
    if target.visibility != Visibility::Internal {
        return Err(PromoteInternalCandidateError::PublicApplication {
            application_id: target.application_id.to_string(),
        });
    }

    Ok(())
}

// Converts structured health failures into durable deployment diagnostics.
fn health_failure_message(failure: &HealthCheckFailure) -> String {
    match failure {
        HealthCheckFailure::TimedOut => "internal health check timed out".to_owned(),
        HealthCheckFailure::Unreachable { kind } => {
            format!("internal health endpoint was unreachable: {kind:?}")
        }
        HealthCheckFailure::InvalidResponse => {
            "internal health endpoint returned an invalid HTTP response".to_owned()
        }
        HealthCheckFailure::UnexpectedStatus { expected, actual } => {
            format!("internal health endpoint returned status {actual}; expected {expected}")
        }
    }
}
