use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::adapters::local_runtime::ObservedRuntimeState;
use crate::domain::manifest::Visibility;
use crate::use_cases::deployment_create::{DeploymentStatus, RuntimeState};

#[derive(Debug, PartialEq, Eq)]
pub struct PublicExposureTarget {
    pub application_id: String,
    pub domain: String,
}

#[derive(Debug, PartialEq, Eq)]
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
            | Self::InvalidDiagnostic => None,
        }
    }
}

pub fn begin_public_exposure(
    connection: &Connection,
    runtime_id: &str,
) -> Result<PublicExposureTarget, PromotePublicCandidateError> {
    let target = load_target(connection, runtime_id)?;
    validate_runtime(&target)?;
    if target.deployment_status != DeploymentStatus::Activating {
        return Err(PromotePublicCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id,
            actual: target.deployment_status.database_value().to_owned(),
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
    if target.visibility != Visibility::Public {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id,
            reason: format!("visibility is `{}`", target.visibility.database_value()),
        });
    }

    let updated = connection
        .execute(
            "UPDATE exposures
             SET materialization_state = 'applying',
                 last_error_code = NULL,
                 last_error_message = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1 AND desired_visibility = 'public'",
            [&target.application_id],
        )
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    if updated != 1 {
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

pub fn record_public_exposure_failure(
    connection: &Connection,
    application_id: &str,
    code: &str,
    message: &str,
    outcome: ExposureOutcome,
) -> Result<(), PromotePublicCandidateError> {
    if !is_trimmed_nonempty(code) || !is_trimmed_nonempty(message) {
        return Err(PromotePublicCandidateError::InvalidDiagnostic);
    }
    let state = match outcome {
        ExposureOutcome::Failed => "failed",
        ExposureOutcome::Diverged => "diverged",
    };
    let updated = connection
        .execute(
            "UPDATE exposures
             SET materialization_state = ?1,
                 last_error_code = ?2,
                 last_error_message = ?3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?4 AND desired_visibility = 'public'",
            params![state, code, message, application_id],
        )
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    if updated != 1 {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: application_id.to_owned(),
            reason: "public exposure was not found".to_owned(),
        });
    }
    Ok(())
}

pub fn promote_public_candidate(
    connection: &mut Connection,
    runtime_id: &str,
    configuration_version: &str,
) -> Result<PromotedPublicCandidate, PromotePublicCandidateError> {
    if !is_trimmed_nonempty(configuration_version) {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: "unknown".to_owned(),
            reason: "configuration version must be trimmed and non-empty".to_owned(),
        });
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    let target = load_target(&transaction, runtime_id)?;
    validate_runtime(&target)?;
    if target.deployment_status != DeploymentStatus::Activating {
        return Err(PromotePublicCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id,
            actual: target.deployment_status.database_value().to_owned(),
        });
    }
    if target.visibility != Visibility::Public || target.domain.is_none() {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id,
            reason: "visibility or domain changed during deployment".to_owned(),
        });
    }

    transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'stopped', updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1
               AND state = 'running'
               AND removed_at IS NULL
               AND id != ?2",
            params![&target.application_id, runtime_id],
        )
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    let runtime_updated = transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'running', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'starting' AND removed_at IS NULL",
            [runtime_id],
        )
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    if runtime_updated != 1 {
        return Err(PromotePublicCandidateError::InvalidRuntime {
            runtime_id: runtime_id.to_owned(),
            reason: "state changed during promotion".to_owned(),
        });
    }
    let exposure_updated = transaction
        .execute(
            "UPDATE exposures
             SET active_runtime_id = ?1,
                 materialization_state = 'active',
                 configuration_version = ?2,
                 last_materialized_at = CURRENT_TIMESTAMP,
                 last_error_code = NULL,
                 last_error_message = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?3
               AND desired_visibility = 'public'
               AND materialization_state = 'applying'",
            params![runtime_id, configuration_version, target.application_id],
        )
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    if exposure_updated != 1 {
        return Err(PromotePublicCandidateError::InvalidExposure {
            application_id: target.application_id,
            reason: "materialization state changed during promotion".to_owned(),
        });
    }
    let deployment_updated = transaction
        .execute(
            "UPDATE deployments
             SET status = 'succeeded',
                 finished_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND status = 'activating'",
            [&target.deployment_id],
        )
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    if deployment_updated != 1 {
        return Err(PromotePublicCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id,
            actual: "changed during promotion".to_owned(),
        });
    }
    transaction
        .execute(
            "UPDATE applications
             SET active_deployment_id = ?1,
                 desired_runtime_state = 'running',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![&target.deployment_id, &target.application_id],
        )
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    let finished_at = transaction
        .query_row(
            "SELECT finished_at FROM deployments WHERE id = ?1",
            [&target.deployment_id],
            |row| row.get(0),
        )
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;
    transaction
        .commit()
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?;

    Ok(PromotedPublicCandidate {
        runtime_id: runtime_id.to_owned(),
        deployment_id: target.deployment_id,
        finished_at,
    })
}

struct PromotionTarget {
    runtime_id: String,
    application_id: String,
    deployment_id: String,
    state: RuntimeState,
    observed_state: ObservedRuntimeState,
    removed_at: Option<String>,
    deployment_status: DeploymentStatus,
    visibility: Visibility,
    domain: Option<String>,
}

fn load_target(
    connection: &Connection,
    runtime_id: &str,
) -> Result<PromotionTarget, PromotePublicCandidateError> {
    connection
        .query_row(
            "SELECT
                runtime_instances.application_id,
                runtime_instances.deployment_id,
                runtime_instances.state,
                runtime_instances.last_observed_state,
                runtime_instances.removed_at,
                deployments.status,
                exposures.desired_visibility,
                exposures.domain
             FROM runtime_instances
             JOIN deployments ON deployments.id = runtime_instances.deployment_id
             JOIN exposures ON exposures.application_id = runtime_instances.application_id
             WHERE runtime_instances.id = ?1",
            [runtime_id],
            |row| {
                let state_text: String = row.get(2)?;
                let state = RuntimeState::from_database(&state_text).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("invalid runtime state: {state_text}"),
                        )),
                    )
                })?;
                let observed_state_text: String = row.get(3)?;
                let observed_state = ObservedRuntimeState::from_database(&observed_state_text);
                let status_text: String = row.get(5)?;
                let deployment_status =
                    DeploymentStatus::from_database(&status_text).ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("invalid deployment status: {status_text}"),
                            )),
                        )
                    })?;
                let visibility_text: String = row.get(6)?;
                let visibility = Visibility::from_database(&visibility_text).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("invalid visibility: {visibility_text}"),
                        )),
                    )
                })?;
                Ok(PromotionTarget {
                    runtime_id: runtime_id.to_owned(),
                    application_id: row.get(0)?,
                    deployment_id: row.get(1)?,
                    state,
                    observed_state,
                    removed_at: row.get(4)?,
                    deployment_status,
                    visibility,
                    domain: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(|source| PromotePublicCandidateError::Persistence { source })?
        .ok_or_else(|| PromotePublicCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_owned(),
        })
}

fn validate_runtime(target: &PromotionTarget) -> Result<(), PromotePublicCandidateError> {
    let reason = if target.state != RuntimeState::Starting {
        Some(format!("state is `{}`", target.state.database_value()))
    } else if target.observed_state != ObservedRuntimeState::Running {
        Some(format!(
            "observed state is `{}`",
            target.observed_state.database_value()
        ))
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

fn is_trimmed_nonempty(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}
