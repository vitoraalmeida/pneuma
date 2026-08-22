use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::health_check_internal::{
    HealthCheckError, HealthCheckResult, check_internal_health,
};
use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::domain::deployment::{
    DeploymentEvent, PromotedCandidate, PromotionCandidateRejection, PromotionTarget,
};
use crate::domain::exposure::Visibility;
use crate::domain::identity::RuntimeInstanceId;
use crate::domain::runtime::HealthCheckSpecification;
use crate::use_cases::deployment_transition::{TransitionDeploymentError, fail_deployment};

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
    ApplicationStore {
        source: ApplicationStoreError,
    },
    RuntimeStore {
        source: RuntimeStoreError,
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
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to promote internal candidate: {source}")
            }
            Self::RuntimeStore { source } => {
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
            Self::ApplicationStore { source } => Some(source),
            Self::RuntimeStore { source } => Some(source),
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
    runtime_id: &RuntimeInstanceId,
    health_check: &HealthCheckSpecification,
) -> Result<PromotedCandidate, PromoteInternalCandidateError> {
    let target = load_target(connection, runtime_id)?;
    if let Some(promoted) = target.completed_promotion() {
        return Ok(promoted);
    }
    target
        .validate_promotion_candidate()
        .map_err(|rejection| match rejection {
            PromotionCandidateRejection::NotStarting { actual } => {
                PromoteInternalCandidateError::InvalidRuntimeState {
                    runtime_id: target.runtime_id.to_string(),
                    actual: actual.to_string(),
                }
            }
            PromotionCandidateRejection::NotRunning { actual } => {
                PromoteInternalCandidateError::RuntimeNotRunning {
                    runtime_id: target.runtime_id.to_string(),
                    actual: actual.to_string(),
                }
            }
            PromotionCandidateRejection::Removed => PromoteInternalCandidateError::RuntimeRemoved {
                runtime_id: target.runtime_id.to_string(),
            },
        })?;
    ensure_activation_ready(&target)?;
    if target.visibility != Visibility::Internal {
        return Err(PromoteInternalCandidateError::PublicApplication {
            application_id: target.application_id.to_string(),
        });
    }

    let health = check_internal_health(target.endpoint.socket_addr(), health_check)
        .map_err(|source| PromoteInternalCandidateError::HealthCheck { source })?;
    match health {
        HealthCheckResult::Healthy { .. } => {}
        HealthCheckResult::Unhealthy { ref failure, .. } => {
            let message = failure.to_string();
            fail_deployment(
                connection,
                &target.deployment_id,
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
    if let Some(promoted) = target.completed_promotion() {
        transaction
            .commit()
            .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
        return Ok(promoted);
    }
    target
        .validate_promotion_candidate()
        .map_err(|rejection| match rejection {
            PromotionCandidateRejection::NotStarting { actual } => {
                PromoteInternalCandidateError::InvalidRuntimeState {
                    runtime_id: target.runtime_id.to_string(),
                    actual: actual.to_string(),
                }
            }
            PromotionCandidateRejection::NotRunning { actual } => {
                PromoteInternalCandidateError::RuntimeNotRunning {
                    runtime_id: target.runtime_id.to_string(),
                    actual: actual.to_string(),
                }
            }
            PromotionCandidateRejection::Removed => PromoteInternalCandidateError::RuntimeRemoved {
                runtime_id: target.runtime_id.to_string(),
            },
        })?;
    ensure_activation_ready(&target)?;
    if target.visibility != Visibility::Internal {
        return Err(PromoteInternalCandidateError::PublicApplication {
            application_id: target.application_id.to_string(),
        });
    }

    runtime_store::stop_other_running_runtimes(
        &transaction,
        &target.application_id,
        &target.runtime_id,
    )
    .map_err(|source| PromoteInternalCandidateError::RuntimeStore { source })?;
    if runtime_store::start_runtime(&transaction, &target.runtime_id)
        .map_err(|source| PromoteInternalCandidateError::RuntimeStore { source })?
        == PersistenceOutcome::Stale
    {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: "changed during promotion".to_owned(),
        });
    }
    if deployment_store::mark_succeeded(
        &transaction,
        &target.deployment_id,
        target.deployment_status,
    )
    .map_err(|source| PromoteInternalCandidateError::Store { source })?
        == PersistenceOutcome::Stale
    {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: "changed during promotion".to_owned(),
        });
    }
    if application_store::activate_deployment(
        &transaction,
        &target.application_id,
        &target.deployment_id,
    )
    .map_err(|source| PromoteInternalCandidateError::ApplicationStore { source })?
        == PersistenceOutcome::Stale
    {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: "changed during promotion".to_owned(),
        });
    }
    let finished_at = deployment_store::load_finished_at(&transaction, &target.deployment_id)
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

// Asks the domain whether the loaded deployment may record its candidate activation.
fn ensure_activation_ready(target: &PromotionTarget) -> Result<(), PromoteInternalCandidateError> {
    target
        .deployment_status
        .transition(DeploymentEvent::Activated)
        .map_err(|_| PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: target.deployment_status.to_string(),
        })?;
    Ok(())
}

// Loads and validates persisted state text before making promotion decisions.
fn load_target(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PromotionTarget, PromoteInternalCandidateError> {
    deployment_store::load_promotion_target(connection, runtime_id)
        .map_err(|source| PromoteInternalCandidateError::Store { source })?
        .ok_or_else(|| PromoteInternalCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_string(),
        })
}
