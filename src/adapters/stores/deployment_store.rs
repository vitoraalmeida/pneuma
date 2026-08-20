use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::domain::deployment::{
    Deployment, DeploymentFailure, DeploymentFailureEvidence, DeploymentHistory,
    DeploymentLifecycle, DeploymentStatus, DeploymentType, SourceRevision,
};
use crate::domain::identity::{ApplicationId, DeploymentId, ReleaseId};
use crate::domain::release::{OciArtifact, Release};

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
    InvalidEvidence {
        deployment_id: String,
        reason: String,
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
            Self::InvalidEvidence {
                deployment_id,
                reason,
            } => write!(
                formatter,
                "deployment `{deployment_id}` has invalid lifecycle evidence: {reason}"
            ),
            Self::Persistence { source } => write!(formatter, "deployment store error: {source}"),
        }
    }
}

impl Error for DeploymentStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            _ => None,
        }
    }
}

// Allocates a deployment ID inside the transaction that reserves the Application for activation.
pub fn generate_id(connection: &Connection) -> Result<String, DeploymentStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(persistence)
}

// Loads the in-progress Deployment that currently reserves an Application, if any.
pub fn load_nonterminal_deployment(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<Option<Deployment>, DeploymentStoreError> {
    let deployment = transaction.query_row(
        "SELECT id, application_id, release_id, type, status, source_revision, requested_at, started_at,
                finished_at, failure_code, failure_stage, failure_message
         FROM deployments WHERE application_id = ?1
           AND status NOT IN ('succeeded', 'failed')",
        [application_id], raw_deployment_from_row,
    ).optional().map_err(persistence)?;
    deployment.map(RawDeployment::into_deployment).transpose()
}

// Finds the Release of the active Deployment only when its runtime is still live.
pub fn load_active_runtime_release_id(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<Option<String>, DeploymentStoreError> {
    transaction.query_row(
        "SELECT d.release_id FROM deployments d JOIN applications a ON a.active_deployment_id = d.id
         WHERE a.id = ?1 AND EXISTS (
             SELECT 1 FROM runtime_instances ri WHERE ri.deployment_id = d.id
               AND ri.state IN ('running', 'stopped') AND ri.removed_at IS NULL
         )", [application_id], |row| row.get(0),
    ).optional().map_err(persistence)
}

// Persists a new activation attempt in its initial pending state.
pub fn insert_pending_deployment(
    transaction: &Transaction<'_>,
    deployment_id: &str,
    application_id: &str,
    release_id: &str,
    deployment_type: DeploymentType,
    source_revision: Option<&SourceRevision>,
) -> Result<(), DeploymentStoreError> {
    transaction.execute(
        "INSERT INTO deployments (id, application_id, release_id, type, status, source_revision)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
        params![deployment_id, application_id, release_id, deployment_type.database_value(), source_revision.map(SourceRevision::as_str)],
    ).map_err(persistence)?;
    Ok(())
}

// Hydrates one deployment and validates its lifecycle evidence matrix.
pub fn load_deployment(
    transaction: &Transaction<'_>,
    deployment_id: &str,
) -> Result<Deployment, DeploymentStoreError> {
    let deployment = transaction.query_row(
        "SELECT id, application_id, release_id, type, status, source_revision, requested_at, started_at,
                finished_at, failure_code, failure_stage, failure_message
         FROM deployments WHERE id = ?1", [deployment_id], raw_deployment_from_row,
    ).optional().map_err(persistence)?.ok_or_else(|| DeploymentStoreError::NotFound { deployment_id: deployment_id.to_owned() })?;
    deployment.into_deployment()
}

// Returns history with typed deployment, immutable release, and persisted active marker.
pub fn list_deployment_history(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Vec<DeploymentHistory>, DeploymentStoreError> {
    let mut statement = connection.prepare(
        "SELECT d.id, d.application_id, d.release_id, d.type, d.status, d.source_revision, d.requested_at, d.started_at,
                d.finished_at, d.failure_code, d.failure_stage, d.failure_message,
                r.application_id, r.image_reference, r.image_repository, r.image_digest, r.created_at,
                COALESCE(d.id = a.active_deployment_id, 0)
         FROM deployments d JOIN releases r ON r.id = d.release_id
         LEFT JOIN applications a ON a.id = d.application_id
         WHERE d.application_id = ?1 ORDER BY d.requested_at DESC",
    ).map_err(persistence)?;
    let rows = statement
        .query_map([application_id.as_str()], |row| {
            let deployment = deployment_from_row(row)?;
            let image_reference: String = row.get(13)?;
            let repository: String = row.get(14)?;
            let digest: String = row.get(15)?;
            let artifact = OciArtifact::from_persisted(&image_reference, &repository, &digest)
                .map_err(|error| conversion_error(13, error))?;
            Ok(DeploymentHistory {
                release: Release {
                    id: deployment.release_id.clone(),
                    application_id: ApplicationId::from(row.get::<_, String>(12)?),
                    artifact,
                    created_at: row.get(16)?,
                },
                deployment,
                is_active: row.get(17)?,
            })
        })
        .map_err(persistence)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(persistence)
}

// Loads the current Deployment status for transition and recovery decisions.
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
        .map_err(persistence)?
        .ok_or_else(|| DeploymentStoreError::NotFound {
            deployment_id: deployment_id.to_owned(),
        })?;
    DeploymentStatus::from_database(&status).ok_or_else(|| DeploymentStoreError::InvalidStatus {
        deployment_id: deployment_id.to_owned(),
        status,
    })
}

// Advances Deployment status with compare-and-set semantics and timestamps its first start.
pub fn advance_status(
    connection: &Connection,
    deployment_id: &str,
    expected: DeploymentStatus,
    next: DeploymentStatus,
) -> Result<bool, DeploymentStoreError> {
    let updated = connection.execute(
        "UPDATE deployments SET status = ?1, started_at = CASE WHEN status = 'pending' THEN CURRENT_TIMESTAMP ELSE started_at END,
         updated_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status = ?3",
        params![next.database_value(), deployment_id, expected.database_value()],
    ).map_err(persistence)?;
    Ok(updated == 1)
}

// Records complete terminal failure evidence only from the supplied in-progress stage.
pub fn mark_failed(
    transaction: &Transaction<'_>,
    deployment_id: &str,
    stage: DeploymentStatus,
    code: &str,
    message: &str,
) -> Result<DeploymentFailure, DeploymentStoreError> {
    transaction.execute(
        "UPDATE deployments SET status = 'failed', finished_at = CURRENT_TIMESTAMP, failure_code = ?1,
         failure_stage = ?2, failure_message = ?3, updated_at = CURRENT_TIMESTAMP WHERE id = ?4 AND status = ?2",
        params![code, stage.database_value(), message, deployment_id],
    ).map_err(persistence)?;
    let finished_at: String = transaction
        .query_row(
            "SELECT finished_at FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| row.get(0),
        )
        .map_err(persistence)?;
    DeploymentFailure::new(code, stage, message, finished_at).map_err(|error| {
        DeploymentStoreError::InvalidEvidence {
            deployment_id: deployment_id.to_owned(),
            reason: error.to_string(),
        }
    })
}

// Marks a Deployment successful only when its expected prior stage still holds.
pub fn mark_succeeded(
    transaction: &Transaction<'_>,
    deployment_id: &str,
    expected_status: DeploymentStatus,
) -> Result<bool, DeploymentStoreError> {
    let updated = transaction.execute(
        "UPDATE deployments SET status = 'succeeded', finished_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1 AND status = ?2", params![deployment_id, expected_status.database_value()],
    ).map_err(persistence)?;
    Ok(updated == 1)
}

// Reads the terminal timestamp persisted by a completed Deployment transition.
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
        .map_err(persistence)
}

fn deployment_from_row(row: &Row<'_>) -> rusqlite::Result<Deployment> {
    raw_deployment_from_row(row)?
        .into_deployment()
        .map_err(|error| conversion_error(4, error))
}

struct RawDeployment {
    id: String,
    application_id: String,
    release_id: String,
    deployment_type: String,
    status: String,
    source_revision: Option<String>,
    requested_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    failure_code: Option<String>,
    failure_stage: Option<String>,
    failure_message: Option<String>,
}

fn raw_deployment_from_row(row: &Row<'_>) -> rusqlite::Result<RawDeployment> {
    Ok(RawDeployment {
        id: row.get(0)?,
        application_id: row.get(1)?,
        release_id: row.get(2)?,
        deployment_type: row.get(3)?,
        status: row.get(4)?,
        source_revision: row.get(5)?,
        requested_at: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
        failure_code: row.get(9)?,
        failure_stage: row.get(10)?,
        failure_message: row.get(11)?,
    })
}

impl RawDeployment {
    fn into_deployment(self) -> Result<Deployment, DeploymentStoreError> {
        let deployment_type =
            DeploymentType::from_database(&self.deployment_type).ok_or_else(|| {
                DeploymentStoreError::InvalidType {
                    deployment_id: self.id.clone(),
                    deployment_type: self.deployment_type,
                }
            })?;
        let status = DeploymentStatus::from_database(&self.status).ok_or_else(|| {
            DeploymentStoreError::InvalidStatus {
                deployment_id: self.id.clone(),
                status: self.status,
            }
        })?;
        let source_revision = self
            .source_revision
            .map(|value| {
                SourceRevision::from_persisted(&value).map_err(|error| {
                    invalid_evidence(&self.id, &format!("invalid source revision: {error}"))
                })
            })
            .transpose()?;
        let lifecycle = lifecycle_from_values(
            &self.id,
            status,
            self.finished_at,
            self.failure_code,
            self.failure_stage,
            self.failure_message,
        )?;
        Ok(Deployment {
            id: DeploymentId::from(self.id),
            application_id: ApplicationId::from(self.application_id),
            release_id: ReleaseId::from(self.release_id),
            deployment_type,
            lifecycle,
            source_revision,
            requested_at: self.requested_at,
            started_at: self.started_at,
        })
    }
}

fn lifecycle_from_values(
    id: &str,
    status: DeploymentStatus,
    finished_at: Option<String>,
    code: Option<String>,
    stage: Option<String>,
    message: Option<String>,
) -> Result<DeploymentLifecycle, DeploymentStoreError> {
    let has_failure_fields = code.is_some() || stage.is_some() || message.is_some();
    if status.is_nonterminal() {
        if finished_at.is_some() || has_failure_fields {
            return Err(invalid_evidence(
                id,
                "non-terminal status cannot have terminal evidence",
            ));
        }
        return Ok(match status {
            DeploymentStatus::Pending => DeploymentLifecycle::Pending,
            DeploymentStatus::Starting => DeploymentLifecycle::Starting,
            DeploymentStatus::Verifying => DeploymentLifecycle::Verifying,
            DeploymentStatus::Activating => DeploymentLifecycle::Activating,
            DeploymentStatus::Succeeded | DeploymentStatus::Failed => unreachable!(),
        });
    }
    match status {
        DeploymentStatus::Succeeded => {
            if finished_at.as_deref().is_none_or(str::is_empty) || has_failure_fields {
                return Err(invalid_evidence(
                    id,
                    "succeeded status requires only finished_at",
                ));
            }
            Ok(DeploymentLifecycle::Succeeded {
                finished_at: finished_at.unwrap(),
            })
        }
        DeploymentStatus::Failed => match (finished_at, code, stage, message) {
            (Some(finished_at), Some(code), Some(stage), Some(message)) => {
                let stage = DeploymentStatus::from_database(&stage)
                    .ok_or_else(|| invalid_evidence(id, "failure stage is invalid"))?;
                let failure = DeploymentFailure::new(&code, stage, &message, finished_at)
                    .map_err(|error| invalid_evidence(id, &error.to_string()))?;
                Ok(DeploymentLifecycle::Failed {
                    evidence: DeploymentFailureEvidence::Complete(failure),
                })
            }
            _ => Ok(DeploymentLifecycle::Failed {
                evidence: DeploymentFailureEvidence::Incomplete,
            }),
        },
        DeploymentStatus::Pending
        | DeploymentStatus::Starting
        | DeploymentStatus::Verifying
        | DeploymentStatus::Activating => unreachable!(),
    }
}

fn invalid_evidence(deployment_id: &str, reason: &str) -> DeploymentStoreError {
    DeploymentStoreError::InvalidEvidence {
        deployment_id: deployment_id.to_owned(),
        reason: reason.to_owned(),
    }
}
fn persistence(source: rusqlite::Error) -> DeploymentStoreError {
    DeploymentStoreError::Persistence { source }
}
fn conversion_error(index: usize, error: impl Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::{TransactionBehavior, params};

    use crate::adapters::database;
    use crate::domain::deployment::{DeploymentFailureEvidence, DeploymentLifecycle};

    use super::{DeploymentStoreError, load_deployment};

    #[test]
    fn hydrates_complete_failed_evidence() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        seed(
            &connection,
            "failed",
            Some("started"),
            Some("finished"),
            Some("code"),
            Some("starting"),
            Some("message"),
        );
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .unwrap();

        let deployment = load_deployment(&transaction, "deployment").unwrap();

        assert_eq!(deployment.started_at.as_deref(), Some("started"));
        assert!(
            matches!(deployment.lifecycle, DeploymentLifecycle::Failed { evidence: DeploymentFailureEvidence::Complete(ref failure) } if failure.code == "code" && failure.finished_at == "finished")
        );
    }

    #[test]
    fn preserves_incomplete_historical_failed_evidence() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        seed(
            &connection,
            "failed",
            None,
            Some("finished"),
            Some("code"),
            None,
            None,
        );
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .unwrap();

        let deployment = load_deployment(&transaction, "deployment").unwrap();

        assert!(matches!(
            deployment.lifecycle,
            DeploymentLifecycle::Failed {
                evidence: DeploymentFailureEvidence::Incomplete
            }
        ));
    }

    #[test]
    fn rejects_terminal_and_nonterminal_evidence_mismatches() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        seed(
            &connection,
            "starting",
            None,
            Some("finished"),
            None,
            None,
            None,
        );
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .unwrap();

        let error = load_deployment(&transaction, "deployment").unwrap_err();

        assert!(
            matches!(error, DeploymentStoreError::InvalidEvidence { deployment_id, .. } if deployment_id == "deployment")
        );
    }

    fn seed(
        connection: &rusqlite::Connection,
        status: &str,
        started_at: Option<&str>,
        finished_at: Option<&str>,
        code: Option<&str>,
        stage: Option<&str>,
        message: Option<&str>,
    ) {
        connection.execute_batch("INSERT INTO applications (id, name, desired_runtime_state, spec_version, created_at, updated_at) VALUES ('app', 'app', 'stopped', 1, 'now', 'now'); INSERT INTO releases (id, application_id, image_repository, image_digest, image_reference, created_at) VALUES ('release', 'app', 'registry.example/app', 'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'now');").unwrap();
        connection.execute("INSERT INTO deployments (id, application_id, release_id, type, status, requested_at, started_at, finished_at, failure_code, failure_stage, failure_message) VALUES ('deployment', 'app', 'release', 'deploy', ?1, 'requested', ?2, ?3, ?4, ?5, ?6)", params![status, started_at, finished_at, code, stage, message]).unwrap();
    }
}
