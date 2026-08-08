use std::error::Error;
use std::fmt;

use rusqlite::{Connection, params};

use crate::domain::release::Release;

#[derive(Debug)]
pub enum CreateReleaseError {
    ApplicationNotFound { application_id: String },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for CreateReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to create release: {source}")
            }
        }
    }
}

impl Error for CreateReleaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ApplicationNotFound { .. } => None,
            Self::Persistence { source } => Some(source),
        }
    }
}

pub fn create_release(
    connection: &mut Connection,
    application_id: &str,
    image_repository: &str,
    image_digest: &str,
    source_revision: Option<&str>,
) -> Result<Release, CreateReleaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    let application_exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM applications WHERE id = ?1)",
            [application_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|source| CreateReleaseError::Persistence { source })?;
    if !application_exists {
        return Err(CreateReleaseError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        });
    }

    let release_id = transaction
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    transaction
        .execute(
            "INSERT INTO releases (
                id, application_id, image_reference, image_repository, image_digest, source_revision, created_at
             ) VALUES (?1, ?2, ?3 || ':' || ?4, ?3, ?4, ?5, CURRENT_TIMESTAMP)
             ON CONFLICT(application_id, image_digest) DO NOTHING",
            params![
                release_id,
                application_id,
                image_repository,
                image_digest,
                source_revision
            ],
        )
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    let release = transaction
        .query_row(
            "SELECT id, application_id, image_reference, image_repository, image_digest, source_revision, created_at
             FROM releases
             WHERE application_id = ?1 AND image_digest = ?2",
            params![application_id, image_digest],
            |row| {
                Ok(Release {
                    id: row.get(0)?,
                    application_id: row.get(1)?,
                    image_reference: row.get(2)?,
                    image_repository: row.get(3)?,
                    image_digest: row.get(4)?,
                    source_revision: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    transaction
        .commit()
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    Ok(release)
}
