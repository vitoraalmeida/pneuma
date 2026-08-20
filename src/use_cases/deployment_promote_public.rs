use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use crate::domain::deployment::DeploymentStatus;
use crate::domain::exposure::{
    DomainName, ExposureConfigurationVersion, ExposureDiagnostic, ExposureMaterializationState,
    Visibility,
};
use crate::domain::runtime::{ObservedRuntimeState, RuntimeState};

#[derive(Debug, PartialEq, Eq)]
// Supplies the route identity after public exposure enters the applying state.
pub struct PublicExposureTarget {
    pub application_id: String,
    pub domain: DomainName,
}

#[derive(Debug, PartialEq, Eq)]
// Identifies a candidate whose runtime, route, and deployment were atomically promoted.
pub struct PromotedPublicCandidate {
    pub runtime_id: String,
    pub deployment_id: String,
    pub finished_at: String,
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
    runtime_id: &str,
) -> Result<PublicExposureTarget, PromotePublicCandidateError> {
    let target = load_target(connection, runtime_id)?;
    validate_runtime(&target)?;
    if target.deployment_status != DeploymentStatus::Activating {
        return Err(PromotePublicCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id,
            actual: target.deployment_status.to_string(),
        });
    }
    let domain =
        target
            .domain
            .clone()
            .ok_or_else(|| PromotePublicCandidateError::InvalidExposure {
                application_id: target.application_id.clone(),
                reason: "public visibility requires a domain".to_owned(),
            })?;
    let domain =
        DomainName::new(&domain).map_err(|_| PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id.clone(),
            reason: "public visibility has an invalid domain".to_owned(),
        })?;
    if target.visibility != Visibility::Public {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id,
            reason: format!("visibility is `{}`", target.visibility),
        });
    }

    let updated = crate::adapters::stores::application_store::begin_public_exposure(
        connection,
        &target.application_id,
    )
    .map_err(|source| PromotePublicCandidateError::Persistence {
        source: match source {
            crate::adapters::stores::application_store::ApplicationStoreError::Persistence {
                source,
            } => source,
            _ => rusqlite::Error::QueryReturnedNoRows,
        },
    })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id,
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
    application_id: &str,
    diagnostic: &ExposureDiagnostic,
    outcome: ExposureOutcome,
) -> Result<(), PromotePublicCandidateError> {
    let state = match outcome {
        ExposureOutcome::Failed => ExposureMaterializationState::Failed,
        ExposureOutcome::Diverged => ExposureMaterializationState::Diverged,
    };
    let updated = crate::adapters::stores::application_store::record_public_exposure_failure(
        connection,
        application_id,
        diagnostic,
        state,
    )
    .map_err(|source| PromotePublicCandidateError::Persistence {
        source: match source {
            crate::adapters::stores::application_store::ApplicationStoreError::Persistence {
                source,
            } => source,
            _ => rusqlite::Error::QueryReturnedNoRows,
        },
    })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: application_id.to_owned(),
            reason: "public exposure was not found".to_owned(),
        });
    }
    Ok(())
}

// Atomically confirms a previously materialized and externally healthy public candidate.
pub fn promote_public_candidate(
    connection: &mut Connection,
    runtime_id: &str,
    configuration_version: &ExposureConfigurationVersion,
) -> Result<PromotedPublicCandidate, PromotePublicCandidateError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    let target = load_target(&transaction, runtime_id)?;
    validate_runtime(&target)?;
    if target.deployment_status != DeploymentStatus::Activating {
        return Err(PromotePublicCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id,
            actual: target.deployment_status.to_string(),
        });
    }
    if target.visibility != Visibility::Public
        || !target
            .domain
            .as_deref()
            .is_some_and(|domain| DomainName::new(domain).is_ok())
    {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id,
            reason: "visibility or domain changed during deployment".to_owned(),
        });
    }

    let outcome = crate::adapters::stores::deployment_store::promote_public(
        &transaction,
        &target,
        configuration_version,
    )
    .map_err(|source| PromotePublicCandidateError::Persistence {
        source: match source {
            crate::adapters::stores::deployment_store::DeploymentStoreError::Persistence {
                source,
            } => source,
            _ => rusqlite::Error::QueryReturnedNoRows,
        },
    })?;
    if outcome == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: runtime_id.to_owned(),
            reason: "state changed during promotion".to_owned(),
        });
    }
    let finished_at = crate::adapters::stores::deployment_store::load_finished_at(
        &transaction,
        &target.deployment_id,
    )
    .map_err(|source| PromotePublicCandidateError::Persistence {
        source: match source {
            crate::adapters::stores::deployment_store::DeploymentStoreError::Persistence {
                source,
            } => source,
            _ => rusqlite::Error::QueryReturnedNoRows,
        },
    })?;
    transaction
        .commit()
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;

    Ok(PromotedPublicCandidate {
        runtime_id: runtime_id.to_owned(),
        deployment_id: target.deployment_id,
        finished_at,
    })
}

// Captures persisted runtime, deployment, and exposure facts for public promotion.
type PromotionTarget = crate::adapters::stores::deployment_store::PromotionTarget;

// Loads the promotion target so later checks can reject incompatible state before promotion writes.
fn load_target(
    connection: &Connection,
    runtime_id: &str,
) -> Result<PromotionTarget, PromotePublicCandidateError> {
    crate::adapters::stores::deployment_store::load_promotion_target(connection, runtime_id)
        .map_err(|source| PromotePublicCandidateError::Persistence {
            source: match source {
                crate::adapters::stores::deployment_store::DeploymentStoreError::Persistence {
                    source,
                } => source,
                _ => rusqlite::Error::QueryReturnedNoRows,
            },
        })?
        .ok_or_else(|| PromotePublicCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_owned(),
        })
}

// Requires the candidate to remain observed running and not retired before promotion.
fn validate_runtime(target: &PromotionTarget) -> Result<(), PromotePublicCandidateError> {
    let reason = if target.state != RuntimeState::Starting {
        Some(format!("state is `{}`", target.state))
    } else if target.observed_state != ObservedRuntimeState::Running {
        Some(format!("observed state is `{}`", target.observed_state))
    } else if target.removed_at.is_some() {
        Some("runtime has been removed".to_owned())
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: target.runtime_id.clone(),
            reason,
        });
    }
    Ok(())
}
