use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::domain::deployment::{DeploymentFailure, DeploymentStatus, InvalidDeploymentFailure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentTransition {
    Start,
    RuntimeRunning,
    Verified,
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
    InvalidPersistedType {
        deployment_id: String,
        deployment_type: String,
    },
    InvalidFailure {
        source: InvalidDeploymentFailure,
    },
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
            Self::InvalidPersistedType {
                deployment_id,
                deployment_type,
            } => write!(
                formatter,
                "deployment `{deployment_id}` has invalid persisted type `{deployment_type}`"
            ),
            Self::InvalidFailure { source } => write!(formatter, "{source}"),
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
            | Self::InvalidPersistedType { .. }
            | Self::InvalidFailure { .. } => None,
        }
    }
}

impl From<DeploymentStoreError> for TransitionDeploymentError {
    fn from(error: DeploymentStoreError) -> Self {
        match error {
            DeploymentStoreError::NotFound { deployment_id } => {
                Self::DeploymentNotFound { deployment_id }
            }
            DeploymentStoreError::Stale { deployment_id } => Self::InvalidPersistedStatus {
                deployment_id,
                status: "changed before persistence".to_owned(),
            },
            DeploymentStoreError::InvalidStatus {
                deployment_id,
                status,
            } => Self::InvalidPersistedStatus {
                deployment_id,
                status,
            },
            DeploymentStoreError::InvalidType {
                deployment_id,
                deployment_type,
            } => Self::InvalidPersistedType {
                deployment_id,
                deployment_type,
            },
            DeploymentStoreError::InvalidEvidence {
                deployment_id,
                reason,
            } => Self::InvalidPersistedStatus {
                deployment_id,
                status: reason,
            },
            DeploymentStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

// Advances one expected state with compare-and-set semantics to detect concurrent changes.
pub fn advance_deployment(
    connection: &Connection,
    deployment_id: &str,
    transition: DeploymentTransition,
) -> Result<DeploymentStatus, TransitionDeploymentError> {
    let (expected, next) = transition_states(transition);
    let advanced = deployment_store::advance_status(connection, deployment_id, expected, next)?;
    match advanced {
        PersistenceOutcome::Updated => return Ok(next),
        PersistenceOutcome::Stale => {}
    }

    let actual = deployment_store::load_status(connection, deployment_id)?;
    Err(TransitionDeploymentError::Conflict {
        deployment_id: deployment_id.to_owned(),
        expected,
        actual,
    })
}

// Atomically records a terminal failure only while the deployment remains non-terminal.
pub fn fail_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    code: &str,
    message: &str,
) -> Result<DeploymentFailure, TransitionDeploymentError> {
    DeploymentFailure::validate_details(code, DeploymentStatus::Pending, message)
        .map_err(|source| TransitionDeploymentError::InvalidFailure { source })?;

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| TransitionDeploymentError::Persistence { source })?;
    let stage = deployment_store::load_status(&transaction, deployment_id)?;
    if !stage.is_nonterminal() {
        return Err(TransitionDeploymentError::CannotFail {
            deployment_id: deployment_id.to_owned(),
            actual: stage,
        });
    }
    DeploymentFailure::validate_details(code, stage, message)
        .map_err(|source| TransitionDeploymentError::InvalidFailure { source })?;

    let failure = deployment_store::mark_failed(&transaction, deployment_id, stage, code, message)?;
    transaction
        .commit()
        .map_err(|source| TransitionDeploymentError::Persistence { source })?;

    Ok(failure)
}

// Defines the closed deployment state-machine edges accepted by this use case.
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
