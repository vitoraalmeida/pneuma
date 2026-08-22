use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use super::transition::{TransitionDeploymentError, fail_deployment};
use crate::adapters::health_check_internal::{
    HealthCheckError, HealthCheckResult, check_internal_health,
};
use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::exposure_store::{self, ExposureStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::domain::deployment::{
    DeploymentEvent, DeploymentStatus, PromotedCandidate, PromotionCandidateRejection,
    PromotionTarget,
};
use crate::domain::exposure::{
    ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent, ExposureOutcome,
    PublicExposureTarget, Visibility,
};
use crate::domain::identity::{ApplicationId, RuntimeInstanceId};
use crate::domain::runtime::HealthCheckSpecification;

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

#[derive(Debug)]
pub(crate) enum PromotePublicCandidateError {
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
            | Self::InvalidExposure { .. } => None,
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
pub(crate) fn promote_public_candidate(
    connection: &mut Connection,
    runtime_id: &RuntimeInstanceId,
    configuration_version: &ExposureConfigurationVersion,
) -> Result<PromotedCandidate, PromotePublicCandidateError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
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
        target.deployment_status,
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
fn load_public_target(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PromotionTarget, PromotePublicCandidateError> {
    crate::adapters::stores::deployment_store::load_promotion_target(connection, runtime_id)
        .map_err(|source| PromotePublicCandidateError::DeploymentStore { source })?
        .ok_or_else(|| PromotePublicCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_string(),
        })
}
