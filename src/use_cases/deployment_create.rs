use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::release_store::{self, ReleaseStoreError};
use crate::domain::deployment::{Deployment, DeploymentType};

#[derive(Debug)]
pub enum CreateDeploymentError {
    ReleaseNotFound { release_id: String },
    ApplicationNotFound { application_id: String },
    ActiveDeployment { application_id: String },
    AlreadyActive { release_id: String },
    ApplicationStore { source: ApplicationStoreError },
    ReleaseStore { source: ReleaseStoreError },
    DeploymentStore { source: DeploymentStoreError },
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
            Self::ActiveDeployment { application_id } => write!(
                formatter,
                "application `{application_id}` already has an active deployment"
            ),
            Self::AlreadyActive { release_id } => write!(
                formatter,
                "release `{release_id}` is already the active deployment"
            ),
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to create deployment: {source}")
            }
            Self::ReleaseStore { source } => {
                write!(formatter, "failed to create deployment: {source}")
            }
            Self::DeploymentStore { source } => {
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
            Self::ReleaseStore { source } => Some(source),
            Self::DeploymentStore { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::ReleaseNotFound { .. }
            | Self::ApplicationNotFound { .. }
            | Self::ActiveDeployment { .. }
            | Self::AlreadyActive { .. } => None,
        }
    }
}

pub fn create_deployment(
    connection: &mut Connection,
    application_id: &str,
    release_id: &str,
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

pub fn create_deployment_with_source_revision(
    connection: &mut Connection,
    application_id: &str,
    release_id: &str,
    deployment_type: DeploymentType,
    source_revision: Option<&str>,
) -> Result<Deployment, CreateDeploymentError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| CreateDeploymentError::Persistence { source })?;

    let application_exists = application_store::application_exists(&transaction, application_id)
        .map_err(|source| CreateDeploymentError::ApplicationStore { source })?;
    if !application_exists {
        return Err(CreateDeploymentError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        });
    }
    let release_exists = release_store::release_exists(&transaction, release_id, application_id)
        .map_err(|source| CreateDeploymentError::ReleaseStore { source })?;
    if !release_exists {
        return Err(CreateDeploymentError::ReleaseNotFound {
            release_id: release_id.to_owned(),
        });
    }
    let has_nonterminal =
        deployment_store::has_nonterminal_deployment(&transaction, application_id)
            .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    if has_nonterminal {
        return Err(CreateDeploymentError::ActiveDeployment {
            application_id: application_id.to_owned(),
        });
    }
    let active_release_id =
        deployment_store::load_active_runtime_release_id(&transaction, application_id)
            .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    if active_release_id.as_deref() == Some(release_id) && deployment_type == DeploymentType::Deploy
    {
        return Err(CreateDeploymentError::AlreadyActive {
            release_id: release_id.to_owned(),
        });
    }

    let deployment_id = deployment_store::generate_id(&transaction)
        .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    deployment_store::insert_pending_deployment(
        &transaction,
        &deployment_id,
        application_id,
        release_id,
        deployment_type,
        source_revision,
    )
    .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;
    let deployment = deployment_store::load_deployment(&transaction, &deployment_id)
        .map_err(|source| CreateDeploymentError::DeploymentStore { source })?;

    transaction
        .commit()
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    Ok(deployment)
}
