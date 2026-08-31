use std::error::Error;

use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

use crate::adapters::application_lock::{ApplicationLock, ApplicationLockError};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::domain::deployment::Deployment;
use crate::domain::deployment::DeploymentType;
use crate::domain::git::CommitSha;
use crate::domain::identity::{ApplicationId, ReleaseId};

#[derive(Debug, Error)]
pub enum CreateDeploymentError {
    #[error("failed to acquire deployment lock: {source}")]
    ApplicationLock {
        #[source]
        source: ApplicationLockError,
    },
    #[error("application `{application_id}` already has an operation in progress")]
    ApplicationBusy { application_id: String },
    #[error("release `{release_id}` was not found")]
    ReleaseNotFound { release_id: String },
    #[error("application `{application_id}` was not found")]
    ApplicationNotFound { application_id: String },
    #[error(
        "application `{}` already has an active deployment",
        deployment.application_id
    )]
    ActiveDeployment { deployment: Box<Deployment> },
    #[error("release `{release_id}` is already the active deployment")]
    AlreadyActive { release_id: String },
    #[error("failed to create deployment: {source}")]
    Persistence {
        #[source]
        source: Box<dyn Error>,
    },
}

impl From<rusqlite::Error> for CreateDeploymentError {
    fn from(source: rusqlite::Error) -> Self {
        Self::Persistence {
            source: Box::new(source),
        }
    }
}

impl From<ApplicationStoreError> for CreateDeploymentError {
    fn from(error: ApplicationStoreError) -> Self {
        Self::Persistence {
            source: Box::new(error),
        }
    }
}

impl From<DeploymentStoreError> for CreateDeploymentError {
    fn from(error: DeploymentStoreError) -> Self {
        Self::Persistence {
            source: Box::new(error),
        }
    }
}

// Creates a deployment without source provenance for callers that deploy an existing release.
pub fn create_deployment(
    connection: &mut Connection,
    application_id: &ApplicationId,
    release_id: &ReleaseId,
    deployment_type: DeploymentType,
) -> Result<Deployment, CreateDeploymentError> {
    create_deployment_with_source_revision(
        connection,
        application_id,
        release_id,
        deployment_type,
        None,
    )
}

// Creates the pending deployment in one transaction while the caller holds the
// application's operation lock.
pub(crate) fn create_deployment_with_source_revision_while_locked(
    connection: &mut Connection,
    application_id: &ApplicationId,
    release_id: &ReleaseId,
    deployment_type: DeploymentType,
    source_revision: Option<&CommitSha>,
) -> Result<Deployment, CreateDeploymentError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let deployment = create_deployment_in_transaction(
        &transaction,
        application_id,
        release_id,
        deployment_type,
        source_revision,
    )?;
    transaction.commit()?;
    Ok(deployment)
}

fn create_deployment_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    application_id: &ApplicationId,
    release_id: &ReleaseId,
    deployment_type: DeploymentType,
    source_revision: Option<&CommitSha>,
) -> Result<Deployment, CreateDeploymentError> {
    let application_exists = application_store::application_exists(transaction, application_id)?;
    if !application_exists {
        return Err(CreateDeploymentError::ApplicationNotFound {
            application_id: application_id.to_string(),
        });
    }
    let release_exists = deployment_store::release_exists(transaction, release_id, application_id)?;
    if !release_exists {
        return Err(CreateDeploymentError::ReleaseNotFound {
            release_id: release_id.to_string(),
        });
    }
    let blocker = deployment_store::load_nonterminal_deployment(transaction, application_id)?;
    if let Some(deployment) = blocker {
        return Err(CreateDeploymentError::ActiveDeployment {
            deployment: Box::new(deployment),
        });
    }
    let active_release_id =
        deployment_store::load_active_runtime_release_id(transaction, application_id)?;
    if active_release_id.as_ref() == Some(release_id) && deployment_type == DeploymentType::Deploy {
        return Err(CreateDeploymentError::AlreadyActive {
            release_id: release_id.to_string(),
        });
    }

    let deployment_id = deployment_store::generate_id(transaction)?;
    deployment_store::insert_pending_deployment(
        transaction,
        &deployment_id,
        application_id,
        release_id,
        deployment_type,
        source_revision,
    )?;
    let deployment = deployment_store::load_deployment(transaction, &deployment_id)?;

    Ok(deployment)
}

// Atomically verifies deployment preconditions and records one pending deployment, preventing
// concurrent active deployments for the same application.
pub fn create_deployment_with_source_revision(
    connection: &mut Connection,
    application_id: &ApplicationId,
    release_id: &ReleaseId,
    deployment_type: DeploymentType,
    source_revision: Option<&CommitSha>,
) -> Result<Deployment, CreateDeploymentError> {
    let Some(_lock) = ApplicationLock::try_acquire_for_connection(connection, application_id)
        .map_err(|source| CreateDeploymentError::ApplicationLock { source })?
    else {
        return Err(CreateDeploymentError::ApplicationBusy {
            application_id: application_id.to_string(),
        });
    };
    create_deployment_with_source_revision_while_locked(
        connection,
        application_id,
        release_id,
        deployment_type,
        source_revision,
    )
}
