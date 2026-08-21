use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::DeploymentStoreError;
use crate::adapters::stores::exposure_store::{self, ExposureStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::domain::deployment::{
    DeploymentStatus, PromotedCandidate, PromotionCandidateRejection, PromotionTarget,
};
use crate::domain::exposure::{
    DomainName, ExposureConfigurationVersion, ExposureDiagnostic, ExposureMaterializationState,
    Visibility,
};
use crate::domain::identity::{ApplicationId, RuntimeInstanceId};

#[derive(Debug, PartialEq, Eq)]
// Supplies the route identity after public exposure enters the applying state.
pub struct PublicExposureTarget {
    pub application_id: ApplicationId,
    pub domain: DomainName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExposureOutcome {
    Failed,
    Diverged,
}

#[derive(Debug)]
pub enum PromotePublicCandidateError {
    RuntimeNotFound {
        runtime_id: String,
    },
    InvalidRuntime {
        runtime_id: String,
        reason: String,
    },
    InvalidDeploymentState {
        deployment_id: String,
        actual: String,
    },
    InvalidExposure {
        application_id: String,
        reason: String,
    },
    InvalidConfigurationVersion,
    InvalidDiagnostic,
    ExposureStore {
        source: ExposureStoreError,
    },
    DeploymentStore {
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

impl fmt::Display for PromotePublicCandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeNotFound { runtime_id } => {
                write!(formatter, "runtime `{runtime_id}` was not found")
            }
            Self::InvalidRuntime { runtime_id, reason } => {
                write!(
                    formatter,
                    "runtime `{runtime_id}` cannot be publicly promoted: {reason}"
                )
            }
            Self::InvalidDeploymentState {
                deployment_id,
                actual,
            } => write!(
                formatter,
                "deployment `{deployment_id}` is `{actual}` during public promotion"
            ),
            Self::InvalidExposure {
                application_id,
                reason,
            } => write!(
                formatter,
                "application `{application_id}` has invalid public exposure: {reason}"
            ),
            Self::InvalidDiagnostic => formatter
                .write_str("exposure failure code and message must be trimmed and non-empty"),
            Self::ExposureStore { source } => {
                write!(formatter, "failed to persist public promotion: {source}")
            }
            Self::DeploymentStore { source } => {
                write!(formatter, "failed to persist public promotion: {source}")
            }
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to persist public promotion: {source}")
            }
            Self::RuntimeStore { source } => {
                write!(formatter, "failed to persist public promotion: {source}")
            }
            Self::InvalidConfigurationVersion => {
                formatter.write_str("exposure configuration version must be non-empty")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to persist public promotion: {source}")
            }
        }
    }
}

impl Error for PromotePublicCandidateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::ExposureStore { source } => Some(source),
            Self::DeploymentStore { source } => Some(source),
            Self::ApplicationStore { source } => Some(source),
            Self::RuntimeStore { source } => Some(source),
            Self::RuntimeNotFound { .. }
            | Self::InvalidRuntime { .. }
            | Self::InvalidDeploymentState { .. }
            | Self::InvalidExposure { .. }
            | Self::InvalidConfigurationVersion
            | Self::InvalidDiagnostic => None,
        }
    }
}

// Marks public exposure as applying before Caddy effects occur outside SQLite transactions.
pub fn begin_public_exposure(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PublicExposureTarget, PromotePublicCandidateError> {
    let target = load_target(connection, runtime_id)?;
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
    let domain =
        target
            .domain
            .clone()
            .ok_or_else(|| PromotePublicCandidateError::InvalidExposure {
                application_id: target.application_id.to_string(),
                reason: "public visibility requires a domain".to_owned(),
            })?;
    if target.visibility != Visibility::Public {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id.to_string(),
            reason: format!("visibility is `{}`", target.visibility),
        });
    }

    let updated = exposure_store::begin_public_exposure(connection, &target.application_id)
        .map_err(|source| PromotePublicCandidateError::ExposureStore { source })?;
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
pub fn record_public_exposure_failure(
    connection: &Connection,
    application_id: &ApplicationId,
    diagnostic: &ExposureDiagnostic,
    outcome: ExposureOutcome,
) -> Result<(), PromotePublicCandidateError> {
    let state = match outcome {
        ExposureOutcome::Failed => ExposureMaterializationState::Failed,
        ExposureOutcome::Diverged => ExposureMaterializationState::Diverged,
    };
    let updated = exposure_store::record_public_exposure_failure(
        connection,
        application_id,
        diagnostic,
        state,
    )
    .map_err(|source| PromotePublicCandidateError::ExposureStore { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: application_id.to_string(),
            reason: "public exposure was not found".to_owned(),
        });
    }
    Ok(())
}

// Atomically confirms a previously materialized and externally healthy public candidate.
pub fn promote_public_candidate(
    connection: &mut Connection,
    runtime_id: &RuntimeInstanceId,
    configuration_version: &ExposureConfigurationVersion,
) -> Result<PromotedCandidate, PromotePublicCandidateError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    let target = load_target(&transaction, runtime_id)?;
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
    if target.visibility != Visibility::Public || target.domain.is_none() {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id.to_string(),
            reason: "visibility or domain changed during deployment".to_owned(),
        });
    }

    runtime_store::stop_other_running_runtimes(
        &transaction,
        &target.application_id,
        &target.runtime_id,
    )
    .map_err(|source| PromotePublicCandidateError::RuntimeStore { source })?;
    if runtime_store::start_runtime(&transaction, &target.runtime_id)
        .map_err(|source| PromotePublicCandidateError::RuntimeStore { source })?
        == PersistenceOutcome::Stale
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
    )
    .map_err(|source| PromotePublicCandidateError::ExposureStore { source })?
        == PersistenceOutcome::Stale
    {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: runtime_id.to_string(),
            reason: "state changed during promotion".to_owned(),
        });
    }
    if crate::adapters::stores::deployment_store::mark_succeeded(
        &transaction,
        &target.deployment_id,
        DeploymentStatus::Activating,
    )
    .map_err(|source| PromotePublicCandidateError::DeploymentStore { source })?
        == PersistenceOutcome::Stale
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
    )
    .map_err(|source| PromotePublicCandidateError::ApplicationStore { source })?
        == PersistenceOutcome::Stale
    {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: runtime_id.to_string(),
            reason: "state changed during promotion".to_owned(),
        });
    }
    let finished_at = crate::adapters::stores::deployment_store::load_finished_at(
        &transaction,
        &target.deployment_id,
    )
    .map_err(|source| PromotePublicCandidateError::DeploymentStore { source })?;
    transaction
        .commit()
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;

    Ok(PromotedCandidate {
        runtime_id: target.runtime_id,
        deployment_id: target.deployment_id,
        finished_at,
    })
}

// Loads the promotion target so later checks can reject incompatible state before promotion writes.
fn load_target(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PromotionTarget, PromotePublicCandidateError> {
    crate::adapters::stores::deployment_store::load_promotion_target(connection, runtime_id)
        .map_err(|source| PromotePublicCandidateError::DeploymentStore { source })?
        .ok_or_else(|| PromotePublicCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_string(),
        })
}
