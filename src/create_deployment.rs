use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior, params};

#[derive(Debug, PartialEq, Eq)]
pub struct Revision {
    pub id: String,
    pub application_id: String,
    pub commit_sha: String,
    pub source_reference: Option<String>,
    pub discovered_at: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Deployment {
    pub id: String,
    pub application_id: String,
    pub revision_id: String,
    pub status: DeploymentStatus,
    pub requested_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentStatus {
    Pending,
    PreparingSource,
    Building,
    Starting,
    VerifyingInternal,
    Succeeded,
    Failed,
}

impl DeploymentStatus {
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::PreparingSource => "preparing_source",
            Self::Building => "building",
            Self::Starting => "starting",
            Self::VerifyingInternal => "verifying_internal",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "preparing_source" => Some(Self::PreparingSource),
            "building" => Some(Self::Building),
            "starting" => Some(Self::Starting),
            "verifying_internal" => Some(Self::VerifyingInternal),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum CreateDeploymentError {
    InvalidCommit,
    ApplicationNotFound { application_id: String },
    ActiveDeployment { application_id: String },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for CreateDeploymentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCommit => formatter.write_str(
                "commit identifier must be a complete 40- or 64-character hexadecimal value",
            ),
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::ActiveDeployment { application_id } => write!(
                formatter,
                "application `{application_id}` already has an active deployment"
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
            Self::InvalidCommit
            | Self::ApplicationNotFound { .. }
            | Self::ActiveDeployment { .. } => None,
        }
    }
}

pub fn create_deployment(
    connection: &mut Connection,
    application_id: &str,
    commit_sha: &str,
    source_reference: Option<&str>,
) -> Result<(Revision, Deployment), CreateDeploymentError> {
    if !is_complete_commit(commit_sha) {
        return Err(CreateDeploymentError::InvalidCommit);
    }

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

    let active_deployment_exists = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM deployments
                WHERE application_id = ?1
                  AND status NOT IN ('succeeded', 'failed', 'rolled_back')
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

    let revision_id = random_id(&transaction)?;
    transaction
        .execute(
            "INSERT INTO revisions (
                id, application_id, commit_sha, source_reference
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(application_id, commit_sha) DO NOTHING",
            params![revision_id, application_id, commit_sha, source_reference],
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    let revision = transaction
        .query_row(
            "SELECT id, application_id, commit_sha, source_reference, discovered_at
             FROM revisions
             WHERE application_id = ?1 AND commit_sha = ?2",
            params![application_id, commit_sha],
            |row| {
                Ok(Revision {
                    id: row.get(0)?,
                    application_id: row.get(1)?,
                    commit_sha: row.get(2)?,
                    source_reference: row.get(3)?,
                    discovered_at: row.get(4)?,
                })
            },
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;

    let deployment_id = random_id(&transaction)?;
    transaction
        .execute(
            "INSERT INTO deployments (
                id, application_id, revision_id, status
             ) VALUES (?1, ?2, ?3, 'pending')",
            params![deployment_id, application_id, revision.id],
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    let deployment = transaction
        .query_row(
            "SELECT id, application_id, revision_id, requested_at
             FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| {
                Ok(Deployment {
                    id: row.get(0)?,
                    application_id: row.get(1)?,
                    revision_id: row.get(2)?,
                    status: DeploymentStatus::Pending,
                    requested_at: row.get(3)?,
                })
            },
        )
        .map_err(|source| CreateDeploymentError::Persistence { source })?;
    transaction
        .commit()
        .map_err(|source| CreateDeploymentError::Persistence { source })?;

    Ok((revision, deployment))
}

fn random_id(transaction: &rusqlite::Transaction<'_>) -> Result<String, CreateDeploymentError> {
    transaction
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| CreateDeploymentError::Persistence { source })
}

fn is_complete_commit(commit_sha: &str) -> bool {
    matches!(commit_sha.len(), 40 | 64) && commit_sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}
