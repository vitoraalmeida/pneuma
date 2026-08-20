use std::error::Error;
use std::fmt;
use std::io;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::domain::identity::{ApplicationId, ReleaseId};
use crate::domain::release::{OciArtifact, Release};

#[derive(Debug)]
pub enum ReleaseStoreError {
    NotFound { release_id: String },
    ApplicationNotFound { application_id: String },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for ReleaseStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { release_id } => {
                write!(formatter, "release `{release_id}` not found")
            }
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` not found")
            }
            Self::Persistence { source } => {
                write!(formatter, "release store error: {source}")
            }
        }
    }
}

impl Error for ReleaseStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::NotFound { .. } | Self::ApplicationNotFound { .. } => None,
        }
    }
}

// Checks that the owning Application exists before Release persistence.
pub fn application_exists(
    connection: &Connection,
    application_id: &str,
) -> Result<bool, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM applications WHERE id = ?1)",
            [application_id],
            |row| row.get(0),
        )
        .map_err(|source| ReleaseStoreError::Persistence { source })
}

// Checks the Application-scoped immutable digest identity used to reuse Releases.
pub fn release_exists_for_digest(
    connection: &Connection,
    application_id: &str,
    image_digest: &str,
) -> Result<bool, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM releases WHERE application_id = ?1 AND image_digest = ?2)",
            params![application_id, image_digest],
            |row| row.get(0),
        )
        .map_err(|source| ReleaseStoreError::Persistence { source })
}

// Allocates a Release ID beside its digest-uniqueness check in the same transaction.
pub fn generate_id(connection: &Connection) -> Result<String, ReleaseStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| ReleaseStoreError::Persistence { source })
}

// Inserts an immutable artifact Release and preserves the existing row for the same digest.
pub fn insert_release(
    transaction: &Transaction<'_>,
    id: &str,
    application_id: &str,
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
                id,
                application_id,
                artifact.reference(),
                artifact.repository(),
                artifact.digest()
            ],
        )
        .map_err(|source| ReleaseStoreError::Persistence { source })?;
    Ok(())
}

// Loads a Release by immutable digest and validates its redundant persisted artifact fields.
pub fn load_release_by_digest(
    connection: &Connection,
    application_id: &str,
    image_digest: &str,
) -> Result<Release, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, image_reference, image_repository,
                    image_digest, created_at
             FROM releases
             WHERE application_id = ?1 AND image_digest = ?2",
            params![application_id, image_digest],
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
                    id: ReleaseId::from(row.get::<_, String>(0)?),
                    application_id: ApplicationId::from(row.get::<_, String>(1)?),
                    artifact,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|source| ReleaseStoreError::Persistence { source })?
        .ok_or_else(|| ReleaseStoreError::NotFound {
            release_id: format!("{application_id}@{image_digest}"),
        })
}

// Loads a Release by durable identity and validates its redundant persisted artifact fields.
pub fn load_release_by_id(
    connection: &Connection,
    release_id: &str,
) -> Result<Release, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, image_reference, image_repository, image_digest, created_at FROM releases WHERE id = ?1",
            [release_id],
            |row| {
                let reference = row.get::<_, String>(2)?;
                let repository = row.get::<_, String>(3)?;
                let digest = row.get::<_, String>(4)?;
                let artifact = artifact_from_values(&reference, &repository, &digest).map_err(|source| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(io::Error::new(io::ErrorKind::InvalidData, source))))?;
                Ok(Release {
                    id: ReleaseId::from(row.get::<_, String>(0)?),
                    application_id: ApplicationId::from(row.get::<_, String>(1)?),
                    artifact,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|source| ReleaseStoreError::Persistence { source })?
        .ok_or_else(|| ReleaseStoreError::NotFound { release_id: release_id.to_owned() })
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

// Checks that a Release belongs to the specified Application before deployment work.
pub fn release_exists(
    connection: &Connection,
    release_id: &str,
    application_id: &str,
) -> Result<bool, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM releases WHERE id = ?1 AND application_id = ?2)",
            params![release_id, application_id],
            |row| row.get(0),
        )
        .map_err(|source| ReleaseStoreError::Persistence { source })
}
