use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

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

#[derive(Debug, PartialEq, Eq)]
pub struct Release {
    pub id: String,
    pub application_id: String,
    pub image_reference: String,
    pub image_repository: String,
    pub image_digest: String,
    pub source_revision: Option<String>,
    pub created_at: String,
}

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

pub fn generate_id(connection: &Connection) -> Result<String, ReleaseStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| ReleaseStoreError::Persistence { source })
}

pub fn insert_release(
    transaction: &Transaction<'_>,
    id: &str,
    application_id: &str,
    image_reference: &str,
    image_repository: &str,
    image_digest: &str,
    source_revision: Option<&str>,
) -> Result<(), ReleaseStoreError> {
    transaction
        .execute(
            "INSERT INTO releases (
                id,
                application_id,
                image_reference,
                image_repository,
                image_digest,
                source_revision,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
            ON CONFLICT(application_id, image_digest) DO NOTHING",
            params![
                id,
                application_id,
                image_reference,
                image_repository,
                image_digest,
                source_revision
            ],
        )
        .map_err(|source| ReleaseStoreError::Persistence { source })?;
    Ok(())
}

pub fn load_release_by_digest(
    connection: &Connection,
    application_id: &str,
    image_digest: &str,
) -> Result<Release, ReleaseStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, image_reference, image_repository,
                    image_digest, source_revision, created_at
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
        .optional()
        .map_err(|source| ReleaseStoreError::Persistence { source })?
        .ok_or_else(|| ReleaseStoreError::NotFound {
            release_id: format!("{application_id}@{image_digest}"),
        })
}

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
