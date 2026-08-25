use std::error::Error;

use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

use super::transition::{TransitionDeploymentError, fail_deployment};
use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::exposure_store::{self, ExposureStoreError};
use crate::adapters::stores::runtime_store;
use crate::domain::deployment::{
    DeploymentEvent, DeploymentStatus, PromotedCandidate, PromotionCandidateRejection,
    PromotionTarget,
};
use crate::domain::exposure::{
    ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent, ExposureOutcome,
    PublicExposureTarget, Visibility,
};
use crate::domain::identity::{ApplicationId, RuntimeInstanceId};
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

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let target = load_target(&transaction, runtime_id)?;
    if let Some(promoted) = target.completed_promotion() {
        transaction.commit()?;
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

#[derive(Debug, Error)]
pub(crate) enum PromotePublicCandidateError {
    #[error("runtime `{runtime_id}` was not found")]
    RuntimeNotFound { runtime_id: String },
    #[error("runtime `{runtime_id}` cannot be publicly promoted: {reason}")]
    InvalidRuntime { runtime_id: String, reason: String },
    #[error("deployment `{deployment_id}` is `{actual}` during public promotion")]
    InvalidDeploymentState {
        deployment_id: String,
        actual: String,
    },
    #[error("application `{application_id}` has invalid public exposure: {reason}")]
    InvalidExposure {
        application_id: String,
        reason: String,
    },
    #[error("failed to persist public promotion: {source}")]
    Persistence {
        #[source]
        source: Box<dyn Error>,
    },
}

impl From<rusqlite::Error> for PromotePublicCandidateError {
    fn from(source: rusqlite::Error) -> Self {
        Self::Persistence {
            source: Box::new(source),
        }
    }
}

impl From<ExposureStoreError> for PromotePublicCandidateError {
    fn from(error: ExposureStoreError) -> Self {
        Self::Persistence {
            source: Box::new(error),
        }
    }
}

impl From<DeploymentStoreError> for PromotePublicCandidateError {
    fn from(error: DeploymentStoreError) -> Self {
        Self::Persistence {
            source: Box::new(error),
        }
    }
}

impl From<ApplicationStoreError> for PromotePublicCandidateError {
    fn from(error: ApplicationStoreError) -> Self {
        Self::Persistence {
            source: Box::new(error),
        }
    }
}

// Marks public exposure as applying before Caddy effects occur outside SQLite transactions.
pub(crate) fn begin_public_exposure(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PublicExposureTarget, PromotePublicCandidateError> {
    let target = load_public_target(connection, runtime_id)?;
    target.validate_promotion_candidate().map_err(|rejection| {
        PromotePublicCandidateError::InvalidRuntime {
            runtime_id: target.runtime_id.to_string(),
            reason: match rejection {
                PromotionCandidateRejection::NotStarting { actual } => {
                    format!("state is `{actual}`")
                }
                PromotionCandidateRejection::NotRunning { actual } => {
                    format!("observed state is `{actual}`")
                }
                PromotionCandidateRejection::Removed => "runtime has been removed".to_owned(),
            },
        }
    })?;
    if target.deployment_status != DeploymentStatus::Activating {
        return Err(PromotePublicCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: target.deployment_status.to_string(),
        });
    }
    let domain = match ExposureIntent::new(target.visibility, target.domain.clone()) {
        Ok(ExposureIntent::Public { domain }) => domain,
        Ok(ExposureIntent::Internal { .. }) => {
            return Err(PromotePublicCandidateError::InvalidExposure {
                application_id: target.application_id.to_string(),
                reason: format!("visibility is `{}`", target.visibility),
            });
        }
        Err(error) => {
            return Err(PromotePublicCandidateError::InvalidExposure {
                application_id: target.application_id.to_string(),
                reason: error.reason,
            });
        }
    };

    let updated = exposure_store::begin_public_exposure(connection, &target.application_id)?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id.to_string(),
            reason: "exposure changed while application was being prepared".to_owned(),
        });
    }

    Ok(PublicExposureTarget {
        application_id: target.application_id,
        domain,
    })
}

// Records whether failed public-route compensation left a safe or diverged state.
pub(crate) fn record_public_exposure_failure(
    connection: &Connection,
    application_id: &ApplicationId,
    diagnostic: &ExposureDiagnostic,
    outcome: ExposureOutcome,
) -> Result<(), PromotePublicCandidateError> {
    let updated = exposure_store::record_public_exposure_failure(
        connection,
        application_id,
        diagnostic,
        outcome.state(),
    )?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: application_id.to_string(),
            reason: "public exposure was not found".to_owned(),
        });
    }
    Ok(())
}

// Atomically confirms a previously materialized and externally healthy public candidate.
pub(crate) fn promote_public_candidate(
    connection: &mut Connection,
    runtime_id: &RuntimeInstanceId,
    configuration_version: &ExposureConfigurationVersion,
) -> Result<PromotedCandidate, PromotePublicCandidateError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let target = load_public_target(&transaction, runtime_id)?;
    target.validate_promotion_candidate().map_err(|rejection| {
        PromotePublicCandidateError::InvalidRuntime {
            runtime_id: target.runtime_id.to_string(),
            reason: match rejection {
                PromotionCandidateRejection::NotStarting { actual } => {
                    format!("state is `{actual}`")
                }
                PromotionCandidateRejection::NotRunning { actual } => {
                    format!("observed state is `{actual}`")
                }
                PromotionCandidateRejection::Removed => "runtime has been removed".to_owned(),
            },
        }
    })?;
    target
        .deployment_status
        .transition(DeploymentEvent::Activated)
        .map_err(|_| PromotePublicCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.to_string(),
            actual: target.deployment_status.to_string(),
        })?;
    if !matches!(
        ExposureIntent::new(target.visibility, target.domain.clone()),
        Ok(ExposureIntent::Public { .. })
    ) {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id.to_string(),
            reason: "visibility or domain changed during deployment".to_owned(),
        });
    }

    runtime_store::stop_other_running_runtimes(
        &transaction,
        &target.application_id,
        &target.runtime_id,
    )?;
    if runtime_store::start_runtime(&transaction, &target.runtime_id)? == PersistenceOutcome::Stale
    {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: runtime_id.to_string(),
            reason: "state changed during promotion".to_owned(),
        });
    }
    if exposure_store::complete_public_exposure_change(
        &transaction,
        &target.application_id,
        &target.runtime_id,
        configuration_version,
    )? == PersistenceOutcome::Stale
    {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: runtime_id.to_string(),
            reason: "state changed during promotion".to_owned(),
        });
    }
    if crate::adapters::stores::deployment_store::mark_succeeded(
        &transaction,
        &target.deployment_id,
        target.deployment_status,
    )? == PersistenceOutcome::Stale
    {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: runtime_id.to_string(),
            reason: "state changed during promotion".to_owned(),
        });
    }
    if application_store::activate_deployment(
        &transaction,
        &target.application_id,
        &target.deployment_id,
    )? == PersistenceOutcome::Stale
    {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: runtime_id.to_string(),
            reason: "state changed during promotion".to_owned(),
        });
    }
    let finished_at = crate::adapters::stores::deployment_store::load_finished_at(
        &transaction,
        &target.deployment_id,
    )?;
    transaction.commit()?;

    Ok(PromotedCandidate {
        runtime_id: target.runtime_id,
        deployment_id: target.deployment_id,
        finished_at,
    })
}

// Loads the promotion target so later checks can reject incompatible state before promotion writes.
fn load_public_target(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PromotionTarget, PromotePublicCandidateError> {
    crate::adapters::stores::deployment_store::load_promotion_target(connection, runtime_id)?
        .ok_or_else(|| PromotePublicCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_string(),
        })
}
