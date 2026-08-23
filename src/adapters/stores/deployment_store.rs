use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::persistence::{
    observed_runtime_state_from_value, outcome, runtime_state_from_value, visibility_from_value,
};
use crate::adapters::stores::release_store::artifact_from_values;
use crate::domain::deployment::{
    Deployment, DeploymentFailure, DeploymentFailureEvidence, DeploymentHistory,
    DeploymentLifecycle, DeploymentStatus, DeploymentType, PromotionTarget, RollbackTarget,
    SourceRevision,
};
use crate::domain::exposure::DomainName;
use crate::domain::git::CommitSha;
use crate::domain::identity::{ApplicationId, DeploymentId, ReleaseId, RuntimeInstanceId};
use crate::domain::release::Release;
use crate::domain::runtime::{ExpectedRuntimeEndpoint, RuntimeRetirement};
use std::net::{Ipv4Addr, SocketAddr};

#[derive(Debug)]
pub enum DeploymentStoreError {
    NotFound {
        deployment_id: String,
    },
    Stale {
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
            Self::Stale { deployment_id } => {
                write!(
                    formatter,
                    "deployment `{deployment_id}` changed before persistence"
                )
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
pub(crate) fn generate_id(connection: &Connection) -> Result<DeploymentId, DeploymentStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map(DeploymentId::from)
        .map_err(persistence)
}

// Loads the in-progress Deployment that currently reserves an Application, if any.
pub(crate) fn load_nonterminal_deployment(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
) -> Result<Option<Deployment>, DeploymentStoreError> {
    let deployment = transaction.query_row(
        "SELECT id, application_id, release_id, type, status, source_revision, requested_at, started_at,
                finished_at, failure_code, failure_stage, failure_message
         FROM deployments WHERE application_id = ?1
           AND status NOT IN ('succeeded', 'failed')",
        [application_id.as_str()], raw_deployment_from_row,
    ).optional().map_err(persistence)?;
    deployment.map(RawDeployment::into_deployment).transpose()
}

// Finds the Release of the active Deployment only when its runtime is still live.
pub(crate) fn load_active_runtime_release_id(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
) -> Result<Option<ReleaseId>, DeploymentStoreError> {
    transaction.query_row(
        "SELECT d.release_id FROM deployments d JOIN applications a ON a.active_deployment_id = d.id
         WHERE a.id = ?1 AND EXISTS (
             SELECT 1 FROM runtime_instances ri WHERE ri.deployment_id = d.id
               AND ri.state IN ('running', 'stopped') AND ri.removed_at IS NULL
          )", [application_id.as_str()], |row| row.get::<_, String>(0),
    ).optional().map(|release_id| release_id.map(ReleaseId::from)).map_err(persistence)
}

// Confirms that the Release belongs to the Application before creating a Deployment.
pub(crate) fn release_exists(
    transaction: &Transaction<'_>,
    release_id: &ReleaseId,
    application_id: &ApplicationId,
) -> Result<bool, DeploymentStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM releases WHERE id = ?1 AND application_id = ?2)",
            params![release_id.as_str(), application_id.as_str()],
            |row| row.get(0),
        )
        .map_err(persistence)
}

// Persists a new activation attempt in its initial pending state.
pub(crate) fn insert_pending_deployment(
    transaction: &Transaction<'_>,
    deployment_id: &DeploymentId,
    application_id: &ApplicationId,
    release_id: &ReleaseId,
    deployment_type: DeploymentType,
    source_revision: Option<&SourceRevision>,
) -> Result<(), DeploymentStoreError> {
    transaction.execute(
        "INSERT INTO deployments (id, application_id, release_id, type, status, source_revision)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
        params![deployment_id.as_str(), application_id.as_str(), release_id.as_str(), deployment_type_value(deployment_type), source_revision.map(SourceRevision::as_str)],
    ).map_err(persistence)?;
    Ok(())
}

// Hydrates one deployment and validates its lifecycle evidence matrix.
pub(crate) fn load_deployment(
    transaction: &Transaction<'_>,
    deployment_id: &DeploymentId,
) -> Result<Deployment, DeploymentStoreError> {
    let deployment = transaction.query_row(
        "SELECT id, application_id, release_id, type, status, source_revision, requested_at, started_at,
                finished_at, failure_code, failure_stage, failure_message
          FROM deployments WHERE id = ?1", [deployment_id.as_str()], raw_deployment_from_row,
    ).optional().map_err(persistence)?.ok_or_else(|| DeploymentStoreError::NotFound { deployment_id: deployment_id.to_string() })?;
    deployment.into_deployment()
}

// Returns history with typed deployment, immutable release, and persisted active marker.
pub(crate) fn list_deployment_history(
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
            let artifact = artifact_from_values(&image_reference, &repository, &digest)
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
pub(crate) fn load_status(
    connection: &Connection,
    deployment_id: &DeploymentId,
) -> Result<DeploymentStatus, DeploymentStoreError> {
    let status = connection
        .query_row(
            "SELECT status FROM deployments WHERE id = ?1",
            [deployment_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(persistence)?
        .ok_or_else(|| DeploymentStoreError::NotFound {
            deployment_id: deployment_id.to_string(),
        })?;
    deployment_status_from_value(&status).ok_or_else(|| DeploymentStoreError::InvalidStatus {
        deployment_id: deployment_id.to_string(),
        status,
    })
}

// Advances Deployment status with compare-and-set semantics and timestamps its first start.
pub(crate) fn advance_status(
    connection: &Connection,
    deployment_id: &DeploymentId,
    expected: DeploymentStatus,
    next: DeploymentStatus,
) -> Result<PersistenceOutcome, DeploymentStoreError> {
    let updated = connection.execute(
        "UPDATE deployments SET status = ?1, started_at = CASE WHEN status = 'pending' THEN CURRENT_TIMESTAMP ELSE started_at END,
         updated_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status = ?3",
        params![deployment_status_value(next), deployment_id.as_str(), deployment_status_value(expected)],
    ).map_err(persistence)?;
    Ok(outcome(updated))
}

// Records complete terminal failure evidence only from the supplied in-progress stage.
pub(crate) fn mark_failed(
    transaction: &Transaction<'_>,
    deployment_id: &DeploymentId,
    stage: DeploymentStatus,
    code: &str,
    message: &str,
) -> Result<DeploymentFailure, DeploymentStoreError> {
    let updated = transaction.execute(
        "UPDATE deployments SET status = 'failed', finished_at = CURRENT_TIMESTAMP, failure_code = ?1,
         failure_stage = ?2, failure_message = ?3, updated_at = CURRENT_TIMESTAMP WHERE id = ?4 AND status = ?2",
        params![code, deployment_status_value(stage), message, deployment_id.as_str()],
    ).map_err(persistence)?;
    if outcome(updated) == PersistenceOutcome::Stale {
        return Err(DeploymentStoreError::Stale {
            deployment_id: deployment_id.to_string(),
        });
    }
    let finished_at: String = transaction
        .query_row(
            "SELECT finished_at FROM deployments WHERE id = ?1",
            [deployment_id.as_str()],
            |row| row.get(0),
        )
        .map_err(persistence)?;
    DeploymentFailure::new(code, stage, message, finished_at).map_err(|error| {
        DeploymentStoreError::InvalidEvidence {
            deployment_id: deployment_id.to_string(),
            reason: error.to_string(),
        }
    })
}

// Marks a Deployment successful only when its expected prior stage still holds.
pub(crate) fn mark_succeeded(
    transaction: &Transaction<'_>,
    deployment_id: &DeploymentId,
    expected_status: DeploymentStatus,
) -> Result<PersistenceOutcome, DeploymentStoreError> {
    let updated = transaction.execute(
        "UPDATE deployments SET status = 'succeeded', finished_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
          WHERE id = ?1 AND status = ?2", params![deployment_id.as_str(), deployment_status_value(expected_status)],
    ).map_err(persistence)?;
    Ok(outcome(updated))
}

// Reads the terminal timestamp persisted by a completed Deployment transition.
pub(crate) fn load_finished_at(
    transaction: &Transaction<'_>,
    deployment_id: &DeploymentId,
) -> Result<String, DeploymentStoreError> {
    transaction
        .query_row(
            "SELECT finished_at FROM deployments WHERE id = ?1",
            [deployment_id.as_str()],
            |row| row.get(0),
        )
        .map_err(persistence)
}

// Loads all runtime, deployment, and exposure facts required by either promotion path.
pub(crate) fn load_promotion_target(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<Option<PromotionTarget>, DeploymentStoreError> {
    connection.query_row(
        "SELECT ri.application_id, ri.deployment_id, ri.host_port, ri.state, ri.last_observed_state,
                ri.removed_at, d.status, d.finished_at, e.desired_visibility, e.domain
         FROM runtime_instances ri JOIN deployments d ON d.id = ri.deployment_id
         JOIN exposures e ON e.application_id = ri.application_id WHERE ri.id = ?1",
        [runtime_id.as_str()],
        |row| {
            let state_text: String = row.get(3)?;
            let status_text: String = row.get(6)?;
            let visibility_text: String = row.get(8)?;
            let domain = row
                .get::<_, Option<String>>(9)?
                .map(|value| {
                    DomainName::new(&value).map_err(|error| {
                        conversion_error(9, std::io::Error::new(std::io::ErrorKind::InvalidData, error))
                    })
                })
                .transpose()?;
            let endpoint = ExpectedRuntimeEndpoint::new(SocketAddr::from((
                Ipv4Addr::LOCALHOST,
                row.get::<_, u16>(2)?,
            )))
            .map_err(|error| {
                conversion_error(
                    2,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()),
                )
            })?;
            Ok(PromotionTarget {
                runtime_id: runtime_id.clone(),
                application_id: ApplicationId::from(row.get::<_, String>(0)?),
                deployment_id: DeploymentId::from(row.get::<_, String>(1)?),
                endpoint,
                state: runtime_state_from_value(&state_text).ok_or_else(|| conversion_error(3, std::io::Error::new(std::io::ErrorKind::InvalidData, format!("invalid runtime state: {state_text}"))))?,
                observed_state: observed_runtime_state_from_value(&row.get::<_, String>(4)?),
                retirement: row.get::<_, Option<String>>(5)?.map(|removed_at| RuntimeRetirement { removed_at }),
                deployment_status: deployment_status_from_value(&status_text).ok_or_else(|| conversion_error(6, std::io::Error::new(std::io::ErrorKind::InvalidData, format!("invalid deployment status: {status_text}"))))?,
                deployment_finished_at: row.get(7)?,
                visibility: visibility_from_value(&visibility_text).ok_or_else(|| conversion_error(8, std::io::Error::new(std::io::ErrorKind::InvalidData, format!("invalid visibility: {visibility_text}"))))?,
                domain,
            })
        },
    ).optional().map_err(persistence)
}

// Selects the most recent succeeded deployment that is no longer active for rollback.
pub(crate) fn load_rollback_target(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Option<RollbackTarget>, DeploymentStoreError> {
    connection.query_row(
        "SELECT r.id, r.application_id, r.image_reference, r.image_repository, r.image_digest, d.source_revision, r.created_at
         FROM deployments d JOIN releases r ON r.id = d.release_id LEFT JOIN applications a ON a.active_deployment_id = d.id
         WHERE d.application_id = ?1 AND d.status = 'succeeded' AND a.id IS NULL ORDER BY d.finished_at DESC LIMIT 1",
        [application_id.as_str()],
        |row| {
            let reference: String = row.get(2)?;
            let repository: String = row.get(3)?;
            let digest: String = row.get(4)?;
            let artifact = artifact_from_values(&reference, &repository, &digest).map_err(|error| conversion_error(2, std::io::Error::new(std::io::ErrorKind::InvalidData, error)))?;
            let source_revision = row.get::<_, Option<String>>(5)?.map(|value| source_revision_from_value(&value).map_err(|error| conversion_error(5, error))).transpose()?;
            Ok(RollbackTarget { release: Release { id: ReleaseId::from(row.get::<_, String>(0)?), application_id: ApplicationId::from(row.get::<_, String>(1)?), artifact, created_at: row.get(6)? }, source_revision })
        },
    ).optional().map_err(persistence)
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
            deployment_type_from_value(&self.deployment_type).ok_or_else(|| {
                DeploymentStoreError::InvalidType {
                    deployment_id: self.id.clone(),
                    deployment_type: self.deployment_type,
                }
            })?;
        let status = deployment_status_from_value(&self.status).ok_or_else(|| {
            DeploymentStoreError::InvalidStatus {
                deployment_id: self.id.clone(),
                status: self.status,
            }
        })?;
        let source_revision = self
            .source_revision
            .map(|value| {
                source_revision_from_value(&value).map_err(|error| {
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
                let stage = deployment_status_from_value(&stage)
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
fn deployment_type_value(value: DeploymentType) -> &'static str {
    match value {
        DeploymentType::Deploy => "deploy",
        DeploymentType::Rollback => "rollback",
    }
}
fn deployment_type_from_value(value: &str) -> Option<DeploymentType> {
    match value {
        "deploy" => Some(DeploymentType::Deploy),
        "rollback" => Some(DeploymentType::Rollback),
        _ => None,
    }
}
fn deployment_status_value(value: DeploymentStatus) -> &'static str {
    match value {
        DeploymentStatus::Pending => "pending",
        DeploymentStatus::Starting => "starting",
        DeploymentStatus::Verifying => "verifying",
        DeploymentStatus::Activating => "activating",
        DeploymentStatus::Succeeded => "succeeded",
        DeploymentStatus::Failed => "failed",
    }
}
fn deployment_status_from_value(value: &str) -> Option<DeploymentStatus> {
    match value {
        "pending" => Some(DeploymentStatus::Pending),
        "starting" => Some(DeploymentStatus::Starting),
        "verifying" => Some(DeploymentStatus::Verifying),
        "activating" => Some(DeploymentStatus::Activating),
        "succeeded" => Some(DeploymentStatus::Succeeded),
        "failed" => Some(DeploymentStatus::Failed),
        _ => None,
    }
}
fn source_revision_from_value(
    value: &str,
) -> Result<SourceRevision, crate::domain::git::InvalidCommitSha> {
    match CommitSha::new(value) {
        Ok(commit) => Ok(SourceRevision::from_commit(commit)),
        Err(_)
            if !value.is_empty()
                && value.trim() == value
                && !value.chars().any(char::is_control) =>
        {
            Ok(SourceRevision::Legacy(value.to_owned()))
        }
        Err(error) => Err(error),
    }
}
fn conversion_error(index: usize, error: impl Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::{TransactionBehavior, params};

    use crate::adapters::database;
    use crate::adapters::stores::PersistenceOutcome;
    use crate::domain::deployment::{
        DeploymentFailureEvidence, DeploymentLifecycle, DeploymentStatus,
    };

    use crate::domain::identity::DeploymentId;

    use super::{DeploymentStoreError, advance_status, load_deployment};

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

        let deployment = load_deployment(&transaction, &DeploymentId::from("deployment")).unwrap();

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

        let deployment = load_deployment(&transaction, &DeploymentId::from("deployment")).unwrap();

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

        let error = load_deployment(&transaction, &DeploymentId::from("deployment")).unwrap_err();

        assert!(
            matches!(error, DeploymentStoreError::InvalidEvidence { deployment_id, .. } if deployment_id == "deployment")
        );
    }

    #[test]
    fn compare_and_set_reports_updated_then_stale() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed(&connection, "pending", None, None, None, None, None);

        assert_eq!(
            advance_status(
                &connection,
                &DeploymentId::from("deployment"),
                DeploymentStatus::Pending,
                DeploymentStatus::Starting,
            )
            .unwrap(),
            PersistenceOutcome::Updated
        );
        assert_eq!(
            advance_status(
                &connection,
                &DeploymentId::from("deployment"),
                DeploymentStatus::Pending,
                DeploymentStatus::Starting,
            )
            .unwrap(),
            PersistenceOutcome::Stale
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
