use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::domain::deployment::DeploymentStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentTransition {
    Start,
    RuntimeRunning,
    Verified,
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

impl From<DeploymentStoreError> for TransitionDeploymentError {
    fn from(error: DeploymentStoreError) -> Self {
        match error {
            DeploymentStoreError::NotFound { deployment_id } => {
                Self::DeploymentNotFound { deployment_id }
            }
            DeploymentStoreError::InvalidStatus {
                deployment_id,
                status,
            } => Self::InvalidPersistedStatus {
                deployment_id,
                status,
            },
            DeploymentStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

pub fn advance_deployment(
    connection: &Connection,
    deployment_id: &str,
    transition: DeploymentTransition,
) -> Result<DeploymentStatus, TransitionDeploymentError> {
    let (expected, next) = transition_states(transition);
    let advanced = deployment_store::advance_status(connection, deployment_id, expected, next)?;
    if advanced {
        return Ok(next);
    }

    let actual = deployment_store::load_status(connection, deployment_id)?;
    Err(TransitionDeploymentError::Conflict {
        deployment_id: deployment_id.to_owned(),
        expected,
        actual,
    })
}

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
    let stage = deployment_store::load_status(&transaction, deployment_id)?;
    if !can_fail(stage) {
        return Err(TransitionDeploymentError::CannotFail {
            deployment_id: deployment_id.to_owned(),
            actual: stage,
        });
    }

    let finished_at =
        deployment_store::mark_failed(&transaction, deployment_id, stage, code, message)?;
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

fn transition_states(transition: DeploymentTransition) -> (DeploymentStatus, DeploymentStatus) {
    match transition {
        DeploymentTransition::Start => (DeploymentStatus::Pending, DeploymentStatus::Starting),
        DeploymentTransition::RuntimeRunning => {
            (DeploymentStatus::Starting, DeploymentStatus::Verifying)
        }
        DeploymentTransition::Verified => {
            (DeploymentStatus::Verifying, DeploymentStatus::Activating)
        }
    }
}

fn can_fail(status: DeploymentStatus) -> bool {
    matches!(
        status,
        DeploymentStatus::Pending
            | DeploymentStatus::Starting
            | DeploymentStatus::Verifying
            | DeploymentStatus::Activating
    )
}

fn is_trimmed_nonempty(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}
