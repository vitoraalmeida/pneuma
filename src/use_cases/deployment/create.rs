use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::operation_store::{self, OperationStoreError};
use crate::domain::deployment::{Deployment, DeploymentType, SourceRevision};
use crate::domain::identity::{ApplicationId, ReleaseId};

#[derive(Debug, Error)]
pub enum CreateDeploymentError {
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
    ApplicationStore {
        #[source]
        source: ApplicationStoreError,
    },
    #[error("failed to create deployment: {source}")]
    DeploymentStore {
        #[source]
        source: DeploymentStoreError,
    },
    #[error("failed to create deployment: {source}")]
    OperationStore {
        #[source]
        source: OperationStoreError,
    },
    #[error("failed to create deployment: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
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

// Creates the pending deployment and replaces operation ownership in one transaction.
pub(crate) fn create_deployment_with_source_revision_and_ownership(
    connection: &mut Connection,
    application_id: &ApplicationId,
    release_id: &ReleaseId,
    deployment_type: DeploymentType,
    source_revision: Option<&SourceRevision>,
    owner_token: Option<&str>,
) -> Result<Deployment, CreateDeploymentError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| CreateDeploymentError::Persistence { source })?;

    let deployment = create_deployment_in_transaction(
        &transaction,
        application_id,
        release_id,
        deployment_type,
        source_revision,
        owner_token,
    )?;
    transaction
        .commit()
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    Ok(deployment)
}

// Atomically verifies deployment preconditions and records one pending deployment, preventing
// concurrent active deployments for the same application.
pub fn create_deployment_with_source_revision(
    connection: &mut Connection,
    application_id: &ApplicationId,
    release_id: &ReleaseId,
    deployment_type: DeploymentType,
    source_revision: Option<&SourceRevision>,
) -> Result<Deployment, CreateDeploymentError> {
    create_deployment_with_source_revision_and_ownership(
        connection,
        application_id,
        release_id,
        deployment_type,
        source_revision,
        None,
    )
}

fn create_deployment_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    application_id: &ApplicationId,
    release_id: &ReleaseId,
    deployment_type: DeploymentType,
    source_revision: Option<&SourceRevision>,
    owner_token: Option<&str>,
) -> Result<Deployment, CreateDeploymentError> {
    let application_exists = application_store::application_exists(transaction, application_id)
        .map_err(|source| CreateDeploymentError::ApplicationStore { source })?;
    if !application_exists {
        return Err(CreateDeploymentError::ApplicationNotFound {
            application_id: application_id.to_string(),
        });
    }
    let release_exists = deployment_store::release_exists(transaction, release_id, application_id)
        .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    if !release_exists {
        return Err(CreateDeploymentError::ReleaseNotFound {
            release_id: release_id.to_string(),
        });
    }
    let blocker = deployment_store::load_nonterminal_deployment(transaction, application_id)
        .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    if let Some(deployment) = blocker {
        return Err(CreateDeploymentError::ActiveDeployment {
            deployment: Box::new(deployment),
        });
    }
    let active_release_id =
        deployment_store::load_active_runtime_release_id(transaction, application_id)
            .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    if active_release_id.as_ref() == Some(release_id) && deployment_type == DeploymentType::Deploy {
        return Err(CreateDeploymentError::AlreadyActive {
            release_id: release_id.to_string(),
        });
    }

    let deployment_id = deployment_store::generate_id(transaction)
        .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    deployment_store::insert_pending_deployment(
        transaction,
        &deployment_id,
        application_id,
        release_id,
        deployment_type,
        source_revision,
    )
    .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    if let Some(owner_token) = owner_token {
        operation_store::take_ownership(transaction, application_id, owner_token)
            .map_err(|source| CreateDeploymentError::OperationStore { source })?;
    }
    let deployment = deployment_store::load_deployment(transaction, &deployment_id)
        .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;

    Ok(deployment)
}
