use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::use_cases::deployment_create::DeploymentStatus;

#[derive(Debug)]
pub struct DeploymentSummary {
    pub id: String,
    pub commit_sha: String,
    pub status: DeploymentStatus,
    pub requested_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug)]
pub enum ListDeploymentsError {
    Persistence { source: rusqlite::Error },
    InvalidStatus { status: String },
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
        }
    }
}

impl Error for ListDeploymentsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::InvalidStatus { .. } => None,
        }
    }
}

pub fn list_deployments(
    connection: &Connection,
    application_id: &str,
) -> Result<Vec<DeploymentSummary>, ListDeploymentsError> {
    let mut statement = connection
        .prepare(
            "SELECT d.id, r.commit_sha, d.status, d.requested_at, d.finished_at
             FROM deployments d
             JOIN revisions r ON r.id = d.revision_id
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
            ))
        })
        .map_err(|source| ListDeploymentsError::Persistence { source })?;

    let mut deployments = Vec::new();
    for row in rows {
        let (id, commit_sha, status_text, requested_at, finished_at) =
            row.map_err(|source| ListDeploymentsError::Persistence { source })?;
        let status = DeploymentStatus::from_database(&status_text).ok_or_else(|| {
            ListDeploymentsError::InvalidStatus {
                status: status_text,
            }
        })?;
        deployments.push(DeploymentSummary {
            id,
            commit_sha,
            status,
            requested_at,
            finished_at,
        });
    }

    Ok(deployments)
}
