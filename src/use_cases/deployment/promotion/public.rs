use std::error::Error;

use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

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
    PublicExposureTarget,
};
use crate::domain::identity::{ApplicationId, RuntimeInstanceId};

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
    ensure_public_runtime_promotable(&target)?;
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
    if updated == PersistenceOutcome::Stale {
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
    if updated == PersistenceOutcome::Stale {
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
    ensure_public_runtime_promotable(&target)?;
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
    if deployment_store::mark_succeeded(
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
    let finished_at = deployment_store::load_finished_at(&transaction, &target.deployment_id)?;
    transaction.commit()?;

    Ok(PromotedCandidate {
        runtime_id: target.runtime_id,
        deployment_id: target.deployment_id,
        finished_at,
    })
}

// Rejects runtimes whose declared or observed state forbids a public promotion write.
fn ensure_public_runtime_promotable(
    target: &PromotionTarget,
) -> Result<(), PromotePublicCandidateError> {
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
    })
}

// Loads the promotion target so later checks can reject incompatible state before promotion writes.
fn load_public_target(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PromotionTarget, PromotePublicCandidateError> {
    deployment_store::load_promotion_target(connection, runtime_id)?.ok_or_else(|| {
        PromotePublicCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_string(),
        }
    })
}
