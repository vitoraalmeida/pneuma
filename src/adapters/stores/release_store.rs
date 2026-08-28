use std::io;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use thiserror::Error;

use crate::adapters::stores::persistence::{entity_id, invalid_text_value};
use crate::domain::identity::{ApplicationId, ReleaseId};
use crate::domain::release::{OciArtifact, Release};

#[derive(Debug, Error)]
pub enum ReleaseStoreError {
    #[error("release `{release_id}` not found")]
    NotFound { release_id: String },
    #[error("release for application `{application_id}` and digest `{image_digest}` not found")]
    NotFoundByArtifact {
        application_id: String,
        image_digest: String,
    },
    #[error("release store error: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

// Allocates a Release ID beside its artifact-uniqueness check in the same transaction.
pub(crate) fn generate_id(connection: &Connection) -> Result<ReleaseId, ReleaseStoreError> {
    let value = connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|source| ReleaseStoreError::Persistence { source })?;
    ReleaseId::new(&value).map_err(|_| ReleaseStoreError::Persistence {
        source: invalid_text_value(0, "release id", &value),
    })
}

// Lists image references for the Releases selected by each Application's active Deployment.
pub(crate) fn active_application_image_references(
    connection: &Connection,
) -> Result<Vec<String>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT releases.image_reference
         FROM applications
         JOIN deployments ON deployments.id = applications.active_deployment_id
         JOIN releases ON releases.id = deployments.release_id",
    )?;
    statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
}

// Inserts an immutable artifact Release and preserves the existing row for the same artifact.
pub(crate) fn insert_release(
    transaction: &Transaction<'_>,
    id: &ReleaseId,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
) -> Result<(), ReleaseStoreError> {
    transaction
        .execute(
            "INSERT INTO releases (id, application_id, image_reference, created_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
             ON CONFLICT(application_id, image_reference) DO NOTHING",
            params![id.as_str(), application_id.as_str(), artifact.reference()],
        )
        .map_err(|source| ReleaseStoreError::Persistence { source })?;
    Ok(())
}

// Loads a Release by immutable digest and validates its canonical persisted artifact.
pub(crate) fn load_release_by_digest(
    connection: &Connection,
    application_id: &ApplicationId,
    image_digest: &str,
) -> Result<Release, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, image_reference, created_at
             FROM releases
             WHERE application_id = ?1 AND image_reference LIKE '%' || ?2",
            params![application_id.as_str(), image_digest],
            |row| {
                Ok(Release {
                    id: entity_id(0, &row.get::<_, String>(0)?)?,
                    application_id: entity_id(1, &row.get::<_, String>(1)?)?,
                    artifact: artifact_from_reference(2, &row.get::<_, String>(2)?)?,
                    created_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|source| ReleaseStoreError::Persistence { source })?
        .ok_or_else(|| ReleaseStoreError::NotFoundByArtifact {
            application_id: application_id.to_string(),
            image_digest: image_digest.to_owned(),
        })
}

// Loads a Release by durable identity and validates its canonical persisted artifact.
pub(crate) fn load_release_by_id(
    connection: &Connection,
    release_id: &ReleaseId,
) -> Result<Release, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, image_reference, created_at
             FROM releases WHERE id = ?1",
            [release_id.as_str()],
            |row| {
                Ok(Release {
                    id: entity_id(0, &row.get::<_, String>(0)?)?,
                    application_id: entity_id(1, &row.get::<_, String>(1)?)?,
                    artifact: artifact_from_reference(2, &row.get::<_, String>(2)?)?,
                    created_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|source| ReleaseStoreError::Persistence { source })?
        .ok_or_else(|| ReleaseStoreError::NotFound {
            release_id: release_id.to_string(),
        })
}

// Hydrates the one canonical artifact column; repository and digest are derived by parsing.
pub(crate) fn artifact_from_reference(
    column: usize,
    reference: &str,
) -> rusqlite::Result<OciArtifact> {
    OciArtifact::parse(reference).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::adapters::database;
    use crate::domain::identity::{ApplicationId, ReleaseId};

    use super::{ReleaseStoreError, load_release_by_digest, load_release_by_id};

    #[test]
    fn missing_digest_lookup_preserves_its_application_and_artifact_context() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        let error = load_release_by_digest(
            &connection,
            &ApplicationId::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
            "sha256:missing",
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ReleaseStoreError::NotFoundByArtifact {
                application_id,
                image_digest,
            } if application_id == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" && image_digest == "sha256:missing"
        ));
    }

    #[test]
    fn a_corrupt_persisted_artifact_reference_fails_hydration() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO systems (id, name) VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'team');
                 INSERT INTO applications (
                     id, system_id, name, repository_url, manifest_path, image_repository,
                     container_port, health_check_path, health_check_expected_status,
                     desired_runtime_state
                 ) VALUES (
                     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'app',
                     'https://example.test/app.git', 'pneuma.toml', 'registry.example/app',
                     8080, '/healthz', 200, 'stopped');
                 INSERT INTO releases (id, application_id, image_reference, created_at)
                 VALUES ('cccccccccccccccccccccccccccccccc', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                         'not a valid reference', 'now');",
            )
            .unwrap();

        let error = load_release_by_id(
            &connection,
            &ReleaseId::new("cccccccccccccccccccccccccccccccc").unwrap(),
        )
        .unwrap_err();

        assert!(matches!(error, ReleaseStoreError::Persistence { .. }));
    }
}
