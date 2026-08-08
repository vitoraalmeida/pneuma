use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

#[derive(Debug, PartialEq, Eq)]
pub struct Deployment {
    pub id: String,
    pub application_id: String,
    pub release_id: String,
    pub deployment_type: DeploymentType,
    pub status: DeploymentStatus,
    pub requested_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentType {
    Deploy,
    Rollback,
}

impl DeploymentType {
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Rollback => "rollback",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "deploy" => Some(Self::Deploy),
            "rollback" => Some(Self::Rollback),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentStatus {
    Pending,
    Starting,
    Verifying,
    Activating,
    Succeeded,
    Failed,
}

impl DeploymentStatus {
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Starting => "starting",
            Self::Verifying => "verifying",
            Self::Activating => "activating",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "starting" => Some(Self::Starting),
            "verifying" => Some(Self::Verifying),
            "activating" => Some(Self::Activating),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Starting,
    Running,
    Stopped,
    Failed,
    Removed,
}

impl RuntimeState {
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Removed => "removed",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "starting" => Some(Self::Starting),
            "running" => Some(Self::Running),
            "stopped" => Some(Self::Stopped),
            "failed" => Some(Self::Failed),
            "removed" => Some(Self::Removed),
            _ => None,
        }
    }
}

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

pub fn create_deployment(
    connection: &mut Connection,
    application_id: &str,
    release_id: &str,
    deployment_type: DeploymentType,
) -> Result<Deployment, CreateDeploymentError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| CreateDeploymentError::Persistence { source })?;

    let application_exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM applications WHERE id = ?1)",
            [application_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    if !application_exists {
        return Err(CreateDeploymentError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        });
    }

    let release_exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM releases WHERE id = ?1 AND application_id = ?2)",
            params![release_id, application_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    if !release_exists {
        return Err(CreateDeploymentError::ReleaseNotFound {
            release_id: release_id.to_owned(),
        });
    }

    let active_deployment_exists = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM deployments
                WHERE application_id = ?1
                  AND status NOT IN ('succeeded', 'failed')
             )",
            [application_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    if active_deployment_exists {
        return Err(CreateDeploymentError::ActiveDeployment {
            application_id: application_id.to_owned(),
        });
    }

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

    let deployment_id = random_id(&transaction)?;
    transaction
        .execute(
            "INSERT INTO deployments (
                id, application_id, release_id, type, status
             ) VALUES (?1, ?2, ?3, ?4, 'pending')",
            params![
                deployment_id,
                application_id,
                release_id,
                deployment_type.database_value()
            ],
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    let deployment = transaction
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
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    transaction
        .commit()
        .map_err(|source| CreateDeploymentError::Persistence { source })?;

    Ok(deployment)
}

fn random_id(transaction: &rusqlite::Transaction<'_>) -> Result<String, CreateDeploymentError> {
    transaction
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| CreateDeploymentError::Persistence { source })
}
