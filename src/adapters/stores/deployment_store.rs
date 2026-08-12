use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::domain::deployment::{Deployment, DeploymentStatus, DeploymentType};

#[derive(Debug)]
pub enum DeploymentStoreError {
    NotFound {
        deployment_id: String,
    },
    InvalidStatus {
        deployment_id: String,
        status: String,
    },
    InvalidType {
        deployment_id: String,
        deployment_type: String,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for DeploymentStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { deployment_id } => {
                write!(formatter, "deployment `{deployment_id}` not found")
            }
            Self::InvalidStatus {
                deployment_id,
                status,
            } => write!(
                formatter,
                "deployment `{deployment_id}` has invalid status `{status}`"
            ),
            Self::InvalidType {
                deployment_id,
                deployment_type,
            } => write!(
                formatter,
                "deployment `{deployment_id}` has invalid type `{deployment_type}`"
            ),
            Self::Persistence { source } => {
                write!(formatter, "deployment store error: {source}")
            }
        }
    }
}

impl Error for DeploymentStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::NotFound { .. } | Self::InvalidStatus { .. } | Self::InvalidType { .. } => None,
        }
    }
}

pub fn generate_id(connection: &Connection) -> Result<String, DeploymentStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| DeploymentStoreError::Persistence { source })
}

pub fn has_nonterminal_deployment(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<bool, DeploymentStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM deployments
                WHERE application_id = ?1
                  AND status NOT IN ('succeeded', 'failed')
             )",
            [application_id],
            |row| row.get(0),
        )
        .map_err(|source| DeploymentStoreError::Persistence { source })
}

pub fn load_active_runtime_release_id(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<Option<String>, DeploymentStoreError> {
    transaction
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
        .map_err(|source| DeploymentStoreError::Persistence { source })
}

pub fn insert_pending_deployment(
    transaction: &Transaction<'_>,
    deployment_id: &str,
    application_id: &str,
    release_id: &str,
    deployment_type: DeploymentType,
    source_revision: Option<&str>,
) -> Result<(), DeploymentStoreError> {
    transaction
        .execute(
            "INSERT INTO deployments (
                id, application_id, release_id, type, status, source_revision
             ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![
                deployment_id,
                application_id,
                release_id,
                deployment_type.database_value(),
                source_revision
            ],
        )
        .map_err(|source| DeploymentStoreError::Persistence { source })?;
    Ok(())
}

pub fn load_deployment(
    transaction: &Transaction<'_>,
    deployment_id: &str,
) -> Result<Deployment, DeploymentStoreError> {
    let (id, application_id, release_id, deployment_type, status, source_revision, requested_at) =
        transaction
            .query_row(
                "SELECT id, application_id, release_id, type, status, source_revision, requested_at
                 FROM deployments WHERE id = ?1",
                [deployment_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|source| DeploymentStoreError::Persistence { source })?
            .ok_or_else(|| DeploymentStoreError::NotFound {
                deployment_id: deployment_id.to_owned(),
            })?;
    let deployment_type = DeploymentType::from_database(&deployment_type).ok_or_else(|| {
        DeploymentStoreError::InvalidType {
            deployment_id: id.clone(),
            deployment_type,
        }
    })?;
    let status = DeploymentStatus::from_database(&status).ok_or_else(|| {
        DeploymentStoreError::InvalidStatus {
            deployment_id: id.clone(),
            status,
        }
    })?;
    Ok(Deployment {
        id,
        application_id,
        release_id,
        deployment_type,
        status,
        source_revision,
        requested_at,
    })
}

pub fn load_status(
    connection: &Connection,
    deployment_id: &str,
) -> Result<DeploymentStatus, DeploymentStoreError> {
    let status = connection
        .query_row(
            "SELECT status FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|source| DeploymentStoreError::Persistence { source })?
        .ok_or_else(|| DeploymentStoreError::NotFound {
            deployment_id: deployment_id.to_owned(),
        })?;
    DeploymentStatus::from_database(&status).ok_or_else(|| DeploymentStoreError::InvalidStatus {
        deployment_id: deployment_id.to_owned(),
        status,
    })
}

pub fn load_deployment_for_registration(
    connection: &Connection,
    deployment_id: &str,
) -> Result<Option<(String, String)>, DeploymentStoreError> {
    connection
        .query_row(
            "SELECT application_id, status FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|source| DeploymentStoreError::Persistence { source })
}

pub fn advance_status(
    connection: &Connection,
    deployment_id: &str,
    expected: DeploymentStatus,
    next: DeploymentStatus,
) -> Result<bool, DeploymentStoreError> {
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
        .map_err(|source| DeploymentStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn mark_failed(
    transaction: &Transaction<'_>,
    deployment_id: &str,
    stage: DeploymentStatus,
    code: &str,
    message: &str,
) -> Result<String, DeploymentStoreError> {
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
        .map_err(|source| DeploymentStoreError::Persistence { source })?;
    transaction
        .query_row(
            "SELECT finished_at FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| row.get(0),
        )
        .map_err(|source| DeploymentStoreError::Persistence { source })
}

pub fn mark_succeeded(
    transaction: &Transaction<'_>,
    deployment_id: &str,
    expected_status: DeploymentStatus,
) -> Result<bool, DeploymentStoreError> {
    let updated = transaction
        .execute(
            "UPDATE deployments
             SET status = 'succeeded',
                 finished_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND status = ?2",
            params![deployment_id, expected_status.database_value()],
        )
        .map_err(|source| DeploymentStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn load_finished_at(
    transaction: &Transaction<'_>,
    deployment_id: &str,
) -> Result<String, DeploymentStoreError> {
    transaction
        .query_row(
            "SELECT finished_at FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| row.get(0),
        )
        .map_err(|source| DeploymentStoreError::Persistence { source })
}
