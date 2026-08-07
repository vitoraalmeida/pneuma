use std::error::Error;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::health_check::{
    HealthCheckError, HealthCheckFailure, HealthCheckResult, check_internal_health,
};
use crate::transition_deployment::{TransitionDeploymentError, fail_deployment};

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
    InvalidRuntimeRole {
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
            Self::InvalidRuntimeRole { runtime_id, actual } => write!(
                formatter,
                "runtime `{runtime_id}` must be Candidate to be promoted, but is `{actual}`"
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
                "deployment `{deployment_id}` must be VerifyingInternal to promote its candidate, but is `{actual}`"
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
            | Self::InvalidRuntimeRole { .. }
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

    // Network I/O must not hold a SQLite write transaction. The target is checked again
    // inside the promotion transaction so a concurrent state change cannot use stale health.
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

    let previous_runtime_id = transaction
        .query_row(
            "SELECT id FROM runtime_instances
             WHERE application_id = ?1
               AND role = 'current'
               AND removed_at IS NULL",
            [&target.application_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
    if let Some(previous_runtime_id) = previous_runtime_id {
        transaction
            .execute(
                "UPDATE runtime_instances
                 SET role = 'previous', updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND role = 'current'",
                [previous_runtime_id],
            )
            .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
    }
    transaction
        .execute(
            "UPDATE runtime_instances
             SET role = 'current', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND role = 'candidate'",
            [runtime_id],
        )
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
    let updated = transaction
        .execute(
            "UPDATE deployments
             SET status = 'succeeded',
                 finished_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND status = 'verifying_internal'",
            [&target.deployment_id],
        )
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
    if updated != 1 {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id,
            actual: "changed during promotion".to_owned(),
        });
    }
    transaction
        .execute(
            "UPDATE applications
             SET desired_runtime_state = 'running', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            [&target.application_id],
        )
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
    let finished_at = transaction
        .query_row(
            "SELECT finished_at FROM deployments WHERE id = ?1",
            [&target.deployment_id],
            |row| row.get(0),
        )
        .map_err(|source| PromoteInternalCandidateError::Persistence { source })?;
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
    role: String,
    observed_state: String,
    removed_at: Option<String>,
    deployment_status: String,
    deployment_finished_at: Option<String>,
    visibility: String,
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
                runtime_instances.role,
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
                Ok(PromotionTarget {
                    runtime_id: runtime_id.to_owned(),
                    application_id: row.get(0)?,
                    deployment_id: row.get(1)?,
                    endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, host_port)),
                    role: row.get(3)?,
                    observed_state: row.get(4)?,
                    removed_at: row.get(5)?,
                    deployment_status: row.get(6)?,
                    deployment_finished_at: row.get(7)?,
                    visibility: row.get(8)?,
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
    if target.role != "current" || target.deployment_status != "succeeded" {
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
    if target.role != "candidate" {
        return Err(PromoteInternalCandidateError::InvalidRuntimeRole {
            runtime_id: target.runtime_id.clone(),
            actual: target.role.clone(),
        });
    }
    if target.observed_state != "running" {
        return Err(PromoteInternalCandidateError::RuntimeNotRunning {
            runtime_id: target.runtime_id.clone(),
            actual: target.observed_state.clone(),
        });
    }
    if target.removed_at.is_some() {
        return Err(PromoteInternalCandidateError::RuntimeRemoved {
            runtime_id: target.runtime_id.clone(),
        });
    }
    if target.deployment_status != "verifying_internal" {
        return Err(PromoteInternalCandidateError::InvalidDeploymentState {
            deployment_id: target.deployment_id.clone(),
            actual: target.deployment_status.clone(),
        });
    }
    if target.visibility != "internal" {
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
