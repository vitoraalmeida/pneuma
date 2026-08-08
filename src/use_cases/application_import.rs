use std::error::Error;
use std::fmt;
use std::path::Path;

use rusqlite::{Connection, params};

use crate::domain::application::Application;
use crate::domain::manifest::{Manifest, ManifestError, Visibility, load_manifest};

const MANIFEST_PATH: &str = "pneuma.toml";

#[derive(Debug)]
pub enum ImportError {
    Manifest { source: ManifestError },
    Persistence { source: rusqlite::Error },
    SystemRequired,
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest { source } => {
                write!(formatter, "failed to import application: {source}")
            }
            Self::Persistence { source } => {
                write!(
                    formatter,
                    "failed to persist imported application: {source}"
                )
            }
            Self::SystemRequired => {
                write!(
                    formatter,
                    "system is required: specify [system] in manifest or use --system flag"
                )
            }
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Manifest { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::SystemRequired => None,
        }
    }
}

pub fn import_application(
    connection: &mut Connection,
    repository_path: &Path,
    system_name: Option<&str>,
) -> Result<Application, ImportError> {
    let manifest =
        load_manifest(repository_path).map_err(|source| ImportError::Manifest { source })?;

    let resolved_system_name = system_name
        .or_else(|| manifest.system.as_ref().map(|s| s.name.as_str()))
        .ok_or(ImportError::SystemRequired)?;

    let transaction = connection
        .transaction()
        .map_err(|source| ImportError::Persistence { source })?;

    let system_id = transaction
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|source| ImportError::Persistence { source })?;

    transaction
        .execute(
            "INSERT INTO systems (id, name, created_at)
             VALUES (?1, ?2, CURRENT_TIMESTAMP)
             ON CONFLICT(name) DO NOTHING",
            params![system_id, resolved_system_name],
        )
        .map_err(|source| ImportError::Persistence { source })?;

    let system_id = transaction
        .query_row(
            "SELECT id FROM systems WHERE name = ?1",
            [resolved_system_name],
            |row| row.get::<_, String>(0),
        )
        .map_err(|source| ImportError::Persistence { source })?;

    let application_id = transaction
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|source| ImportError::Persistence { source })?;
    let inserted = transaction
        .execute(
            "INSERT INTO applications (
                id,
                system_id,
                name,
                desired_runtime_state,
                spec_version,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, 'stopped', ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(name) DO NOTHING",
            params![
                application_id,
                system_id,
                manifest.application.name,
                manifest.schema_version
            ],
        )
        .map_err(|source| ImportError::Persistence { source })?
        == 1;

    if inserted {
        persist_specification(&transaction, &application_id, &manifest)?;
    }

    let application = transaction
        .query_row(
            "SELECT
                applications.id,
                applications.system_id,
                applications.name,
                application_sources.repository_location,
                application_sources.default_branch
             FROM applications
             JOIN application_sources
                ON application_sources.application_id = applications.id
             WHERE applications.name = ?1",
            [&manifest.application.name],
            |row| {
                Ok(Application {
                    id: row.get(0)?,
                    system_id: row.get(1)?,
                    name: row.get(2)?,
                    repository: row.get(3)?,
                    default_branch: row.get(4)?,
                    active_deployment_id: None,
                })
            },
        )
        .map_err(|source| ImportError::Persistence { source })?;

    transaction
        .commit()
        .map_err(|source| ImportError::Persistence { source })?;

    Ok(application)
}

fn persist_specification(
    transaction: &rusqlite::Transaction<'_>,
    application_id: &str,
    manifest: &Manifest,
) -> Result<(), ImportError> {
    let repository_kind = if manifest.source.repository.contains("://") {
        "remote"
    } else {
        "local"
    };
    let visibility = match &manifest.exposure.default_visibility {
        Visibility::Internal => "internal",
        Visibility::Public => "public",
    };

    transaction
        .execute(
            "INSERT INTO application_sources (
                application_id,
                repository_location,
                repository_kind,
                default_branch,
                manifest_path,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                manifest.source.repository,
                repository_kind,
                manifest.source.branch,
                MANIFEST_PATH
            ],
        )
        .map_err(|source| ImportError::Persistence { source })?;
    transaction
        .execute(
            "INSERT INTO application_build_specs (
                application_id,
                containerfile_path,
                context_path,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                manifest.build.containerfile.to_string_lossy(),
                manifest.build.context.to_string_lossy()
            ],
        )
        .map_err(|source| ImportError::Persistence { source })?;
    transaction
        .execute(
            "INSERT INTO application_runtime_specs (
                application_id,
                container_port,
                created_at,
                updated_at
            ) VALUES (?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![application_id, i64::from(manifest.runtime.container_port)],
        )
        .map_err(|source| ImportError::Persistence { source })?;
    transaction
        .execute(
            "INSERT INTO health_check_specs (
                application_id,
                path,
                expected_status,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                manifest.runtime.healthcheck_path,
                i64::from(manifest.runtime.expected_status)
            ],
        )
        .map_err(|source| ImportError::Persistence { source })?;
    transaction
        .execute(
            "INSERT INTO exposures (
                application_id,
                desired_visibility,
                domain,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                visibility,
                manifest.exposure.domain.as_deref()
            ],
        )
        .map_err(|source| ImportError::Persistence { source })?;

    Ok(())
}
