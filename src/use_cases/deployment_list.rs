use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::domain::deployment::{DeploymentStatus, DeploymentType};

#[derive(Debug)]
pub struct DeploymentSummary {
    pub id: String,
    pub release_id: String,
    pub image_reference: String,
    pub image_digest: String,
    pub source_revision: Option<String>,
    pub deployment_type: DeploymentType,
    pub status: DeploymentStatus,
    pub requested_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug)]
pub enum ListDeploymentsError {
    Persistence { source: rusqlite::Error },
    InvalidStatus { status: String },
    InvalidType { deployment_type: String },
}

impl fmt::Display for ListDeploymentsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persistence { source } => {
                write!(formatter, "failed to list deployments: {source}")
            }
            Self::InvalidStatus { status } => {
                write!(formatter, "deployment has invalid status `{status}`")
            }
            Self::InvalidType { deployment_type } => {
                write!(formatter, "deployment has invalid type `{deployment_type}`")
            }
        }
    }
}

impl Error for ListDeploymentsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::InvalidStatus { .. } | Self::InvalidType { .. } => None,
        }
    }
}

pub fn list_deployments(
    connection: &Connection,
    application_id: &str,
) -> Result<Vec<DeploymentSummary>, ListDeploymentsError> {
    let mut statement = connection
        .prepare(
            "SELECT d.id, r.id, r.image_reference, r.image_digest, d.source_revision, d.type, d.status, d.requested_at, d.finished_at
             FROM deployments d
             JOIN releases r ON r.id = d.release_id
             WHERE d.application_id = ?1
             ORDER BY d.requested_at DESC",
        )
        .map_err(|source| ListDeploymentsError::Persistence { source })?;

    let rows = statement
        .query_map([application_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(|source| ListDeploymentsError::Persistence { source })?;

    let mut deployments = Vec::new();
    for row in rows {
        let (
            id,
            release_id,
            image_reference,
            image_digest,
            source_revision,
            type_text,
            status_text,
            requested_at,
            finished_at,
        ) = row.map_err(|source| ListDeploymentsError::Persistence { source })?;
        let status = DeploymentStatus::from_database(&status_text).ok_or_else(|| {
            ListDeploymentsError::InvalidStatus {
                status: status_text,
            }
        })?;
        let deployment_type = DeploymentType::from_database(&type_text).ok_or_else(|| {
            ListDeploymentsError::InvalidType {
                deployment_type: type_text,
            }
        })?;
        deployments.push(DeploymentSummary {
            id,
            release_id,
            image_reference,
            image_digest,
            source_revision,
            deployment_type,
            status,
            requested_at,
            finished_at,
        });
    }

    Ok(deployments)
}
