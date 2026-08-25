use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::domain::deployment::{
    DeploymentEvent, DeploymentFailure, DeploymentStatus, InvalidDeploymentFailure,
    InvalidDeploymentTransition,
};
use crate::domain::identity::DeploymentId;

#[derive(Debug, Error)]
pub enum TransitionDeploymentError {
    #[error("deployment `{deployment_id}` was not found")]
    DeploymentNotFound { deployment_id: String },
    #[error("deployment `{deployment_id}` expected state {expected:?}, but is {actual:?}")]
    Conflict {
        deployment_id: String,
        expected: DeploymentStatus,
        actual: DeploymentStatus,
    },
    #[error("deployment `{deployment_id}` cannot fail from state {actual:?}")]
    CannotFail {
        deployment_id: String,
        actual: DeploymentStatus,
    },
    #[error("deployment `{deployment_id}`: {source}")]
    InvalidTransition {
        deployment_id: String,
        #[source]
        source: InvalidDeploymentTransition,
    },
    #[error("deployment `{deployment_id}` has invalid persisted state `{status}`")]
    InvalidPersistedStatus {
        deployment_id: String,
        status: String,
    },
    #[error("deployment `{deployment_id}` has invalid persisted type `{deployment_type}`")]
    InvalidPersistedType {
        deployment_id: String,
        deployment_type: String,
    },
    #[error(transparent)]
    InvalidFailure { source: InvalidDeploymentFailure },
    #[error("failed to transition deployment: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
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

// Loads the current state, asks the domain for the transition, and persists it under
// compare-and-set so concurrent changes surface as conflicts instead of overwritten state.
pub fn advance_deployment(
    connection: &Connection,
    deployment_id: &DeploymentId,
    event: DeploymentEvent,
) -> Result<DeploymentStatus, TransitionDeploymentError> {
    let current = deployment_store::load_status(connection, deployment_id)?;
    let next = current.transition(event).map_err(|source| {
        TransitionDeploymentError::InvalidTransition {
            deployment_id: deployment_id.to_string(),
            source,
        }
    })?;

    match deployment_store::advance_status(connection, deployment_id, current, next)? {
        PersistenceOutcome::Updated => Ok(next),
        PersistenceOutcome::Stale => {
            let actual = deployment_store::load_status(connection, deployment_id)?;
            Err(TransitionDeploymentError::Conflict {
                deployment_id: deployment_id.to_string(),
                expected: current,
                actual,
            })
        }
    }
}

// Atomically records a terminal failure only while the deployment remains non-terminal.
pub fn fail_deployment(
    connection: &mut Connection,
    deployment_id: &DeploymentId,
    code: &str,
    message: &str,
) -> Result<DeploymentFailure, TransitionDeploymentError> {
    DeploymentFailure::validate_details(code, DeploymentStatus::Pending, message)
        .map_err(|source| TransitionDeploymentError::InvalidFailure { source })?;

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| TransitionDeploymentError::Persistence { source })?;
    let stage = deployment_store::load_status(&transaction, deployment_id)?;
    if !stage.can_fail() {
        return Err(TransitionDeploymentError::CannotFail {
            deployment_id: deployment_id.to_string(),
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
