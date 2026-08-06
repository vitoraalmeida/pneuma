use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::create_deployment::DeploymentStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentTransition {
    Start,
    SourcePrepared,
    ImageBuilt,
    RuntimeRunning,
    InternalVerified,
    TrafficSwitched,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeploymentFailure {
    pub code: String,
    pub stage: DeploymentStatus,
    pub message: String,
    pub finished_at: String,
}

#[derive(Debug)]
pub enum TransitionDeploymentError {
    DeploymentNotFound {
        deployment_id: String,
    },
    Conflict {
        deployment_id: String,
        expected: DeploymentStatus,
        actual: DeploymentStatus,
    },
    CannotFail {
        deployment_id: String,
        actual: DeploymentStatus,
    },
    InvalidPersistedStatus {
        deployment_id: String,
        status: String,
    },
    InvalidFailure,
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for TransitionDeploymentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeploymentNotFound { deployment_id } => {
                write!(formatter, "deployment `{deployment_id}` was not found")
            }
            Self::Conflict {
                deployment_id,
                expected,
                actual,
            } => write!(
                formatter,
                "deployment `{deployment_id}` expected state {expected:?}, but is {actual:?}"
            ),
            Self::CannotFail {
                deployment_id,
                actual,
            } => write!(
                formatter,
                "deployment `{deployment_id}` cannot fail from state {actual:?}"
            ),
            Self::InvalidPersistedStatus {
                deployment_id,
                status,
            } => write!(
                formatter,
                "deployment `{deployment_id}` has invalid persisted state `{status}`"
            ),
            Self::InvalidFailure => formatter
                .write_str("deployment failure code and message must be trimmed and non-empty"),
            Self::Persistence { source } => {
                write!(formatter, "failed to transition deployment: {source}")
            }
        }
    }
}

impl Error for TransitionDeploymentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::DeploymentNotFound { .. }
            | Self::Conflict { .. }
            | Self::CannotFail { .. }
            | Self::InvalidPersistedStatus { .. }
            | Self::InvalidFailure => None,
        }
    }
}

/// Applies one supported lifecycle event only when the persisted state is the event's
/// expected source state. The compare-and-set update prevents retries or competing
/// callers from silently skipping or repeating deployment work.
pub fn advance_deployment(
    connection: &Connection,
    deployment_id: &str,
    transition: DeploymentTransition,
) -> Result<DeploymentStatus, TransitionDeploymentError> {
    let (expected, next) = transition_states(transition);
    let updated = connection
        .execute(
            "UPDATE deployments
             SET status = ?1,
                 started_at = CASE
                    WHEN status = 'pending' THEN CURRENT_TIMESTAMP
                    ELSE started_at
                 END,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2 AND status = ?3",
            params![
                next.database_value(),
                deployment_id,
                expected.database_value()
            ],
        )
        .map_err(|source| TransitionDeploymentError::Persistence { source })?;
    if updated == 1 {
        return Ok(next);
    }

    let actual = load_status(connection, deployment_id)?;
    Err(TransitionDeploymentError::Conflict {
        deployment_id: deployment_id.to_owned(),
        expected,
        actual,
    })
}

/// Records a terminal failure together with the stage that was active when it happened.
/// An immediate transaction keeps the state check, diagnostic fields, and terminal
/// timestamp atomic so another writer cannot advance the deployment in between them.
pub fn fail_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    code: &str,
    message: &str,
) -> Result<DeploymentFailure, TransitionDeploymentError> {
    if !is_trimmed_nonempty(code) || !is_trimmed_nonempty(message) {
        return Err(TransitionDeploymentError::InvalidFailure);
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| TransitionDeploymentError::Persistence { source })?;
    let stage = load_status(&transaction, deployment_id)?;
    if !can_fail(stage) {
        return Err(TransitionDeploymentError::CannotFail {
            deployment_id: deployment_id.to_owned(),
            actual: stage,
        });
    }

    transaction
        .execute(
            "UPDATE deployments
             SET status = 'failed',
                 finished_at = CURRENT_TIMESTAMP,
                 failure_code = ?1,
                 failure_stage = ?2,
                 failure_message = ?3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?4 AND status = ?2",
            params![code, stage.database_value(), message, deployment_id],
        )
        .map_err(|source| TransitionDeploymentError::Persistence { source })?;
    let finished_at = transaction
        .query_row(
            "SELECT finished_at FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| row.get(0),
        )
        .map_err(|source| TransitionDeploymentError::Persistence { source })?;
    transaction
        .commit()
        .map_err(|source| TransitionDeploymentError::Persistence { source })?;

    Ok(DeploymentFailure {
        code: code.to_owned(),
        stage,
        message: message.to_owned(),
        finished_at,
    })
}

// Keeps the allowed source and destination states for each event in one exhaustive
// mapping, so callers cannot choose arbitrary state pairs.
fn transition_states(transition: DeploymentTransition) -> (DeploymentStatus, DeploymentStatus) {
    match transition {
        DeploymentTransition::Start => {
            (DeploymentStatus::Pending, DeploymentStatus::PreparingSource)
        }
        DeploymentTransition::SourcePrepared => (
            DeploymentStatus::PreparingSource,
            DeploymentStatus::Building,
        ),
        DeploymentTransition::ImageBuilt => {
            (DeploymentStatus::Building, DeploymentStatus::Starting)
        }
        DeploymentTransition::RuntimeRunning => (
            DeploymentStatus::Starting,
            DeploymentStatus::VerifyingInternal,
        ),
        DeploymentTransition::InternalVerified => (
            DeploymentStatus::VerifyingInternal,
            DeploymentStatus::SwitchingTraffic,
        ),
        DeploymentTransition::TrafficSwitched => (
            DeploymentStatus::SwitchingTraffic,
            DeploymentStatus::VerifyingExternal,
        ),
    }
}

// Loads and decodes the persisted state so a failed compare-and-set can distinguish a
// missing deployment from a conflicting or unsupported stored state.
fn load_status(
    connection: &Connection,
    deployment_id: &str,
) -> Result<DeploymentStatus, TransitionDeploymentError> {
    let status = connection
        .query_row(
            "SELECT status FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|source| TransitionDeploymentError::Persistence { source })?
        .ok_or_else(|| TransitionDeploymentError::DeploymentNotFound {
            deployment_id: deployment_id.to_owned(),
        })?;
    DeploymentStatus::from_database(&status).ok_or_else(|| {
        TransitionDeploymentError::InvalidPersistedStatus {
            deployment_id: deployment_id.to_owned(),
            status,
        }
    })
}

// Limits failure recording to stages implemented by the current deployment flow,
// preventing a terminal failure or a future promotion state from being overwritten.
fn can_fail(status: DeploymentStatus) -> bool {
    matches!(
        status,
        DeploymentStatus::Pending
            | DeploymentStatus::PreparingSource
            | DeploymentStatus::Building
            | DeploymentStatus::Starting
            | DeploymentStatus::VerifyingInternal
            | DeploymentStatus::SwitchingTraffic
            | DeploymentStatus::VerifyingExternal
    )
}

// Rejects blank or padded diagnostic fields so every terminal failure remains useful
// for machine classification and operator inspection.
fn is_trimmed_nonempty(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}
