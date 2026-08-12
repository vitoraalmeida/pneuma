use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::release_store::{self, ReleaseStoreError};
use crate::domain::deployment::{Deployment, DeploymentStatus, DeploymentType};

#[derive(Debug)]
pub enum CreateDeploymentError {
    ReleaseNotFound { release_id: String },
    ApplicationNotFound { application_id: String },
    ActiveDeployment { application_id: String },
    AlreadyActive { release_id: String },
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
            Self::Persistence { source } => {
                write!(formatter, "failed to create deployment: {source}")
            }
        }
    }
}

impl Error for CreateDeploymentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::ReleaseNotFound { .. }
            | Self::ApplicationNotFound { .. }
            | Self::ActiveDeployment { .. }
            | Self::AlreadyActive { .. } => None,
        }
    }
}

impl From<ApplicationStoreError> for CreateDeploymentError {
    fn from(error: ApplicationStoreError) -> Self {
        match error {
            ApplicationStoreError::NotFound { application_id } => {
                Self::ApplicationNotFound { application_id }
            }
            ApplicationStoreError::SystemNotFound { .. } => Self::ApplicationNotFound {
                application_id: "unknown".to_owned(),
            },
            ApplicationStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

impl From<ReleaseStoreError> for CreateDeploymentError {
    fn from(error: ReleaseStoreError) -> Self {
        match error {
            ReleaseStoreError::NotFound { release_id } => Self::ReleaseNotFound { release_id },
            ReleaseStoreError::ApplicationNotFound { application_id } => {
                Self::ApplicationNotFound { application_id }
            }
            ReleaseStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

impl From<DeploymentStoreError> for CreateDeploymentError {
    fn from(error: DeploymentStoreError) -> Self {
        match error {
            DeploymentStoreError::NotFound { deployment_id } => Self::ReleaseNotFound {
                release_id: deployment_id,
            },
            DeploymentStoreError::InvalidStatus { .. } => Self::Persistence {
                source: rusqlite::Error::QueryReturnedNoRows,
            },
            DeploymentStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

pub fn create_deployment(
    connection: &mut Connection,
    application_id: &str,
    release_id: &str,
    deployment_type: DeploymentType,
) -> Result<Deployment, CreateDeploymentError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| CreateDeploymentError::Persistence { source })?;

    let app_exists = application_store::application_exists(&transaction, application_id)?;
    if !app_exists {
        return Err(CreateDeploymentError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        });
    }

    let rel_exists = release_store::release_exists(&transaction, release_id, application_id)?;
    if !rel_exists {
        return Err(CreateDeploymentError::ReleaseNotFound {
            release_id: release_id.to_owned(),
        });
    }

    check_no_active_deployment(&transaction, application_id)?;
    check_not_already_active(&transaction, application_id, release_id, deployment_type)?;

    let deployment_id = deployment_store::generate_id(&transaction)?;
    insert_deployment(
        &transaction,
        &deployment_id,
        application_id,
        release_id,
        deployment_type,
    )?;
    let deployment = load_deployment(&transaction, &deployment_id)?;

    transaction
        .commit()
        .map_err(|source| CreateDeploymentError::Persistence { source })?;

    Ok(deployment)
}

fn check_no_active_deployment(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<(), CreateDeploymentError> {
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM deployments
                WHERE application_id = ?1
                  AND status NOT IN ('succeeded', 'failed')
             )",
            [application_id],
            |row| row.get(0),
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    if exists {
        return Err(CreateDeploymentError::ActiveDeployment {
            application_id: application_id.to_owned(),
        });
    }
    Ok(())
}

fn check_not_already_active(
    transaction: &Transaction<'_>,
    application_id: &str,
    release_id: &str,
    deployment_type: DeploymentType,
) -> Result<(), CreateDeploymentError> {
    let current_release_id: Option<String> = transaction
        .query_row(
            "SELECT d.release_id FROM deployments d
             JOIN applications a ON a.active_deployment_id = d.id
             WHERE a.id = ?1
               AND EXISTS (
                   SELECT 1 FROM runtime_instances ri
                   WHERE ri.deployment_id = d.id
                     AND ri.state IN ('running', 'stopped')
                     AND ri.removed_at IS NULL
               )",
            [application_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    if current_release_id.as_deref() == Some(release_id)
        && deployment_type == DeploymentType::Deploy
    {
        return Err(CreateDeploymentError::AlreadyActive {
            release_id: release_id.to_owned(),
        });
    }
    Ok(())
}

fn insert_deployment(
    transaction: &Transaction<'_>,
    deployment_id: &str,
    application_id: &str,
    release_id: &str,
    deployment_type: DeploymentType,
) -> Result<(), CreateDeploymentError> {
    transaction
        .execute(
            "INSERT INTO deployments (
                id, application_id, release_id, type, status
             ) VALUES (?1, ?2, ?3, ?4, 'pending')",
            rusqlite::params![
                deployment_id,
                application_id,
                release_id,
                deployment_type.database_value()
            ],
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    Ok(())
}

fn load_deployment(
    transaction: &Transaction<'_>,
    deployment_id: &str,
) -> Result<Deployment, CreateDeploymentError> {
    transaction
        .query_row(
            "SELECT id, application_id, release_id, type, requested_at
             FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| {
                Ok(Deployment {
                    id: row.get(0)?,
                    application_id: row.get(1)?,
                    release_id: row.get(2)?,
                    deployment_type: DeploymentType::from_database(&row.get::<_, String>(3)?)
                        .unwrap_or(DeploymentType::Deploy),
                    status: DeploymentStatus::Pending,
                    requested_at: row.get(4)?,
                })
            },
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })
}
