use std::error::Error;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::adapters::health_check_internal::{
    HealthCheckError, HealthCheckFailure, HealthCheckResult, check_internal_health,
};
use crate::adapters::local_runtime::ObservedRuntimeState;
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::domain::deployment::DeploymentStatus;
use crate::domain::manifest::Visibility;
use crate::domain::runtime::RuntimeState;
use crate::use_cases::deployment_transition::{TransitionDeploymentError, fail_deployment};

#[derive(Debug, PartialEq, Eq)]
pub struct PromotedCandidate {
    pub runtime_id: String,
    pub deployment_id: String,
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

pub fn promote_internal_candidate(
    connection: &mut Connection,
    runtime_id: &str,
    health_path: &str,
    expected_status: u16,
) -> Result<PromotedCandidate, PromoteInternalCandidateError> {
    let target = load_target(connection, runtime_id)?;
    if let Some(promoted) = completed_promotion(&target) {
        return Ok(promoted);
    }
    validate_target(&target)?;

    let health = check_internal_health(target.endpoint, health_path, expected_status)
        .map_err(|source| PromoteInternalCandidateError::HealthCheck { source })?;
    match health {
        HealthCheckResult::Healthy { .. } => {}
        HealthCheckResult::Unhealthy { ref failure, .. } => {
            let message = health_failure_message(failure);
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
    if let Some(promoted) = completed_promotion(&target) {
        transaction
            .commit()
            .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
        return Ok(promoted);
    }
    validate_target(&target)?;

    let previous_runtime_id =
        runtime_store::load_active_runtime_for_application(&transaction, &target.application_id)
            .map_err(|source| PromoteInternalCandidateError::Persistence {
                source: match source {
                    RuntimeStoreError::Persistence { source } => source,
                    _ => rusqlite::Error::QueryReturnedNoRows,
                },
            })?;
    if let Some(previous_runtime_id) = previous_runtime_id {
        runtime_store::stop_runtime(&transaction, &previous_runtime_id).map_err(|source| {
            PromoteInternalCandidateError::Persistence {
                source: match source {
                    RuntimeStoreError::Persistence { source } => source,
                    _ => rusqlite::Error::QueryReturnedNoRows,
                },
            }
        })?;
    }
    runtime_store::start_runtime(&transaction, runtime_id).map_err(|source| {
        PromoteInternalCandidateError::Persistence {
            source: match source {
                RuntimeStoreError::Persistence { source } => source,
                _ => rusqlite::Error::QueryReturnedNoRows,
            },
        }
    })?;
    let updated = deployment_store::mark_succeeded(
        &transaction,
        &target.deployment_id,
        DeploymentStatus::Verifying,
    )
    .map_err(|source| PromoteInternalCandidateError::Persistence {
        source: match source {
            DeploymentStoreError::Persistence { source } => source,
            _ => rusqlite::Error::QueryReturnedNoRows,
        },
    })?;
    if !updated {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id,
            actual: "changed during promotion".to_owned(),
        });
    }
    application_store::activate_deployment(
        &transaction,
        &target.application_id,
        &target.deployment_id,
    )
    .map_err(|source| PromoteInternalCandidateError::Persistence {
        source: match source {
            ApplicationStoreError::Persistence { source } => source,
            _ => rusqlite::Error::QueryReturnedNoRows,
        },
    })?;
    let finished_at = deployment_store::load_finished_at(&transaction, &target.deployment_id)
        .map_err(|source| PromoteInternalCandidateError::Persistence {
            source: match source {
                DeploymentStoreError::Persistence { source } => source,
                _ => rusqlite::Error::QueryReturnedNoRows,
            },
        })?;
    transaction
        .commit()
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;

    Ok(PromotedCandidate {
        runtime_id: runtime_id.to_owned(),
        deployment_id: target.deployment_id,
        finished_at,
    })
}

struct PromotionTarget {
    runtime_id: String,
    application_id: String,
    deployment_id: String,
    endpoint: SocketAddr,
    state: RuntimeState,
    observed_state: ObservedRuntimeState,
    removed_at: Option<String>,
    deployment_status: DeploymentStatus,
    deployment_finished_at: Option<String>,
    visibility: Visibility,
}

fn load_target(
    connection: &Connection,
    runtime_id: &str,
) -> Result<PromotionTarget, PromoteInternalCandidateError> {
    connection
        .query_row(
            "SELECT
                runtime_instances.application_id,
                runtime_instances.deployment_id,
                runtime_instances.host_port,
                runtime_instances.state,
                runtime_instances.last_observed_state,
                runtime_instances.removed_at,
                deployments.status,
                deployments.finished_at,
                exposures.desired_visibility
             FROM runtime_instances
             JOIN deployments ON deployments.id = runtime_instances.deployment_id
             JOIN exposures ON exposures.application_id = runtime_instances.application_id
             WHERE runtime_instances.id = ?1",
            [runtime_id],
            |row| {
                let host_port = row.get::<_, u16>(2)?;
                let state_text: String = row.get(3)?;
                let state = RuntimeState::from_database(&state_text).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("invalid runtime state: {state_text}"),
                        )),
                    )
                })?;
                let observed_state_text: String = row.get(4)?;
                let observed_state = ObservedRuntimeState::from_database(&observed_state_text);
                let status_text: String = row.get(6)?;
                let deployment_status =
                    DeploymentStatus::from_database(&status_text).ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("invalid deployment status: {status_text}"),
                            )),
                        )
                    })?;
                let visibility_text: String = row.get(8)?;
                let visibility = Visibility::from_database(&visibility_text).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
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
                    endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, host_port)),
                    state,
                    observed_state,
                    removed_at: row.get(5)?,
                    deployment_status,
                    deployment_finished_at: row.get(7)?,
                    visibility,
                })
            },
        )
        .optional()
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?
        .ok_or_else(|| PromoteInternalCandidateError::RuntimeNotFound {
            runtime_id: runtime_id.to_owned(),
        })
}

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

fn validate_target(target: &PromotionTarget) -> Result<(), PromoteInternalCandidateError> {
    if target.state != RuntimeState::Starting {
        return Err(PromoteInternalCandidateError::InvalidRuntimeState {
            runtime_id: target.runtime_id.clone(),
            actual: target.state.database_value().to_owned(),
        });
    }
    if target.observed_state != ObservedRuntimeState::Running {
        return Err(PromoteInternalCandidateError::RuntimeNotRunning {
            runtime_id: target.runtime_id.clone(),
            actual: target.observed_state.database_value().to_owned(),
        });
    }
    if target.removed_at.is_some() {
        return Err(PromoteInternalCandidateError::RuntimeRemoved {
            runtime_id: target.runtime_id.clone(),
        });
    }
    if target.deployment_status != DeploymentStatus::Verifying {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.clone(),
            actual: target.deployment_status.database_value().to_owned(),
        });
    }
    if target.visibility != Visibility::Internal {
        return Err(PromoteInternalCandidateError::PublicApplication {
            application_id: target.application_id.clone(),
        });
    }

    Ok(())
}

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
