use std::error::Error;

use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

use super::super::transition::{TransitionDeploymentError, fail_deployment};
use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::runtime_store;
use crate::domain::deployment::{
    DeploymentEvent, DeploymentFailureCode, PromotedCandidate, PromotionCandidateRejection,
    PromotionTarget,
};
use crate::domain::exposure::Visibility;
use crate::domain::identity::RuntimeInstanceId;
use crate::domain::runtime::{HealthCheckSpecification, RuntimeEndpointError};

#[derive(Debug, Error)]
pub enum PromoteInternalCandidateError {
    #[error("runtime `{runtime_id}` was not found")]
    RuntimeNotFound { runtime_id: String },
    #[error("runtime `{runtime_id}` must be Starting to be promoted, but is `{actual}`")]
    InvalidRuntimeState { runtime_id: String, actual: String },
    #[error("runtime `{runtime_id}` must be Running to be promoted, but is `{actual}`")]
    RuntimeNotRunning { runtime_id: String, actual: String },
    #[error("runtime `{runtime_id}` has already been removed")]
    RuntimeRemoved { runtime_id: String },
    #[error(
        "deployment `{deployment_id}` must be Verifying to promote its candidate, but is `{actual}`"
    )]
    InvalidDeploymentState {
        deployment_id: String,
        actual: String,
    },
    #[error("application `{application_id}` requires public route activation before promotion")]
    PublicApplication { application_id: String },
    #[error(transparent)]
    HealthCheck { source: RuntimeEndpointError },
    #[error("candidate failed its internal health check: {result:?}")]
    CandidateUnhealthy { result: HealthCheckResult },
    #[error("failed to record candidate health failure: {source}")]
    RecordFailure {
        #[source]
        source: TransitionDeploymentError,
    },
    #[error("failed to promote internal candidate: {source}")]
    Persistence {
        #[source]
        source: Box<dyn Error>,
    },
}

impl From<rusqlite::Error> for PromoteInternalCandidateError {
    fn from(source: rusqlite::Error) -> Self {
        Self::Persistence {
            source: Box::new(source),
        }
    }
}

impl From<DeploymentStoreError> for PromoteInternalCandidateError {
    fn from(error: DeploymentStoreError) -> Self {
        Self::Persistence {
            source: Box::new(error),
        }
    }
}

impl From<ApplicationStoreError> for PromoteInternalCandidateError {
    fn from(error: ApplicationStoreError) -> Self {
        Self::Persistence {
            source: Box::new(error),
        }
    }
}

// Health-checks an internal candidate outside a transaction, then atomically promotes it.
// The target is validated again inside the transaction because concurrent deployments may
// have changed it between the health check and the promotion writes.
pub fn promote_internal_candidate(
    connection: &mut Connection,
    runtime_id: &RuntimeInstanceId,
    health_check: &HealthCheckSpecification,
) -> Result<PromotedCandidate, PromoteInternalCandidateError> {
    let target = load_target(connection, runtime_id)?;
    if let Some(promoted) = target.completed_promotion() {
        return Ok(promoted);
    }
    ensure_internal_promotable(&target)?;

    let health = check_internal_health(target.endpoint.socket_addr(), health_check)
        .map_err(|source| PromoteInternalCandidateError::HealthCheck { source })?;
    match health {
        HealthCheckResult::Healthy { .. } => {}
        HealthCheckResult::Unhealthy { ref failure, .. } => {
            let message = failure.to_string();
            fail_deployment(
                connection,
                &target.deployment_id,
                DeploymentFailureCode::HealthCheck.as_str(),
                &message,
            )
            .map_err(|source| PromoteInternalCandidateError::RecordFailure { source })?;
            return Err(PromoteInternalCandidateError::CandidateUnhealthy { result: health });
        }
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let target = load_target(&transaction, runtime_id)?;
    if let Some(promoted) = target.completed_promotion() {
        transaction.commit()?;
        return Ok(promoted);
    }
    ensure_internal_promotable(&target)?;

    runtime_store::stop_other_running_runtimes(
        &transaction,
        &target.application_id,
        &target.runtime_id,
    )?;
    if runtime_store::start_runtime(&transaction, &target.runtime_id)? == PersistenceOutcome::Stale
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
    )? == PersistenceOutcome::Stale
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
    )? == PersistenceOutcome::Stale
    {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: "changed during promotion".to_owned(),
        });
    }
    let finished_at = deployment_store::load_finished_at(&transaction, &target.deployment_id)?;
    transaction.commit()?;

    Ok(PromotedCandidate {
        runtime_id: target.runtime_id,
        deployment_id: target.deployment_id,
        finished_at,
    })
}

// Rejects any freshly loaded target that may not proceed to an internal promotion write.
fn ensure_internal_promotable(
    target: &PromotionTarget,
) -> Result<(), PromoteInternalCandidateError> {
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
    ensure_activation_ready(target)?;
    if target.visibility != Visibility::Internal {
        return Err(PromoteInternalCandidateError::PublicApplication {
            application_id: target.application_id.to_string(),
        });
    }
    Ok(())
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
    deployment_store::load_promotion_target(connection, runtime_id)?.ok_or_else(|| {
        PromoteInternalCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_string(),
        }
    })
}
