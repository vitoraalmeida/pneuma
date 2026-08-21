use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::operation_store::{self, OperationStoreError};
use crate::domain::deployment::{Deployment, DeploymentType, SourceRevision};
use crate::domain::identity::{ApplicationId, ReleaseId};

#[derive(Debug)]
pub enum CreateDeploymentError {
    ReleaseNotFound { release_id: String },
    ApplicationNotFound { application_id: String },
    ActiveDeployment { deployment: Box<Deployment> },
    AlreadyActive { release_id: String },
    ApplicationStore { source: ApplicationStoreError },
    DeploymentStore { source: DeploymentStoreError },
    OperationStore { source: OperationStoreError },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for CreateDeploymentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReleaseNotFound { release_id } => {
                write!(formatter, "release `{release_id}` was not found")
            }
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::ActiveDeployment { deployment } => write!(
                formatter,
                "application `{}` already has an active deployment",
                deployment.application_id
            ),
            Self::AlreadyActive { release_id } => write!(
                formatter,
                "release `{release_id}` is already the active deployment"
            ),
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to create deployment: {source}")
            }
            Self::DeploymentStore { source } => {
                write!(formatter, "failed to create deployment: {source}")
            }
            Self::OperationStore { source } => {
                write!(formatter, "failed to create deployment: {source}")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to create deployment: {source}")
            }
        }
    }
}

impl Error for CreateDeploymentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ApplicationStore { source } => Some(source),
            Self::DeploymentStore { source } => Some(source),
            Self::OperationStore { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::ReleaseNotFound { .. }
            | Self::ApplicationNotFound { .. }
            | Self::ActiveDeployment { .. }
            | Self::AlreadyActive { .. } => None,
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

// Creates the pending deployment and replaces operation ownership in one transaction.
pub fn create_deployment_with_source_revision_and_ownership(
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
        operation_store::take_ownership(transaction, application_id.as_str(), owner_token)
            .map_err(|source| CreateDeploymentError::OperationStore { source })?;
    }
    let deployment = deployment_store::load_deployment(transaction, &deployment_id)
        .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;

    Ok(deployment)
}
