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

// Allocates a Release ID beside its digest-uniqueness check in the same transaction.
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

// Inserts an immutable artifact Release and preserves the existing row for the same digest.
pub(crate) fn insert_release(
    transaction: &Transaction<'_>,
    id: &ReleaseId,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
) -> Result<(), ReleaseStoreError> {
    transaction
        .execute(
            "INSERT INTO releases (
                id,
                application_id,
                image_reference,
                image_repository,
                image_digest,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
            ON CONFLICT(application_id, image_digest) DO NOTHING",
            params![
                id.as_str(),
                application_id.as_str(),
                artifact.reference(),
                artifact.repository(),
                artifact.digest()
            ],
        )
        .map_err(|source| ReleaseStoreError::Persistence { source })?;
    Ok(())
}

// Loads a Release by immutable digest and validates its redundant persisted artifact fields.
pub(crate) fn load_release_by_digest(
    connection: &Connection,
    application_id: &ApplicationId,
    image_digest: &str,
) -> Result<Release, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, image_reference, image_repository,
                    image_digest, created_at
             FROM releases
             WHERE application_id = ?1 AND image_digest = ?2",
            params![application_id.as_str(), image_digest],
            |row| {
                let image_reference = row.get::<_, String>(2)?;
                let image_repository = row.get::<_, String>(3)?;
                let image_digest = row.get::<_, String>(4)?;
                let artifact =
                    artifact_from_values(&image_reference, &image_repository, &image_digest)
                        .map_err(|source| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(io::Error::new(io::ErrorKind::InvalidData, source)),
                            )
                        })?;
                Ok(Release {
                    id: entity_id(0, &row.get::<_, String>(0)?)?,
                    application_id: entity_id(1, &row.get::<_, String>(1)?)?,
                    artifact,
                    created_at: row.get(5)?,
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

// Loads a Release by durable identity and validates its redundant persisted artifact fields.
pub(crate) fn load_release_by_id(
    connection: &Connection,
    release_id: &ReleaseId,
) -> Result<Release, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, image_reference, image_repository, image_digest, created_at FROM releases WHERE id = ?1",
            [release_id.as_str()],
            |row| {
                let reference = row.get::<_, String>(2)?;
                let repository = row.get::<_, String>(3)?;
                let digest = row.get::<_, String>(4)?;
                let artifact = artifact_from_values(&reference, &repository, &digest).map_err(|source| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(io::Error::new(io::ErrorKind::InvalidData, source))))?;
                Ok(Release {
                    id: entity_id(0, &row.get::<_, String>(0)?)?,
                    application_id: entity_id(1, &row.get::<_, String>(1)?)?,
                    artifact,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|source| ReleaseStoreError::Persistence { source })?
        .ok_or_else(|| ReleaseStoreError::NotFound { release_id: release_id.to_string() })
}

pub(crate) fn artifact_from_values(
    reference: &str,
    repository: &str,
    digest: &str,
) -> Result<OciArtifact, crate::domain::release::InvalidOciArtifact> {
    let artifact = OciArtifact::parse(reference)?;
    if artifact.repository() != repository || artifact.digest() != digest {
        return Err(crate::domain::release::InvalidOciArtifact {
            reference: reference.to_owned(),
        });
    }
    Ok(artifact)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::adapters::database;
    use crate::domain::identity::ApplicationId;

    use super::{ReleaseStoreError, load_release_by_digest};

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
}
