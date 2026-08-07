use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::create_deployment::DeploymentStatus;

#[derive(Debug)]
pub struct DeploymentSummary {
    pub id: String,
    pub commit_sha: String,
    pub status: DeploymentStatus,
    pub requested_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug)]
pub struct ListDeploymentsError {
    source: rusqlite::Error,
}

impl fmt::Display for ListDeploymentsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to list deployments: {}", self.source)
    }
}

impl Error for ListDeploymentsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
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
        .map_err(|source| ListDeploymentsError { source })?;
    let rows = statement
        .query_map([application_id], |row| {
            let status_text: String = row.get(2)?;
            Ok(DeploymentSummary {
                id: row.get(0)?,
                commit_sha: row.get(1)?,
                status: DeploymentStatus::from_database(&status_text).unwrap(),
                requested_at: row.get(3)?,
                finished_at: row.get(4)?,
            })
        })
        .map_err(|source| ListDeploymentsError { source })?;

    let mut deployments = Vec::new();
    for row in rows {
        deployments.push(row.map_err(|source| ListDeploymentsError { source })?);
    }

    Ok(deployments)
}
