use std::error::Error;
use std::fmt;
use std::path::Path;

use rusqlite::Connection;

use crate::adapters::git_source::is_remote_repository;
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::application::{ApplicationSummary, RepositoryKind};
use crate::domain::manifest::{Manifest, ManifestError, load_manifest_at};

const DEFAULT_MANIFEST_PATH: &str = "pneuma.toml";

#[derive(Debug)]
pub enum ImportError {
    Manifest { source: ManifestError },
    Persistence { source: rusqlite::Error },
    ApplicationNotFound { application_id: String },
    SystemNotFound { system_name: String },
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
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::SystemNotFound { system_name } => {
                write!(formatter, "system `{system_name}` was not found")
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
            Self::ApplicationNotFound { .. }
            | Self::SystemNotFound { .. }
            | Self::SystemRequired => None,
        }
    }
}

impl From<ApplicationStoreError> for ImportError {
    fn from(error: ApplicationStoreError) -> Self {
        match error {
            ApplicationStoreError::Persistence { source } => Self::Persistence { source },
            ApplicationStoreError::NotFound { application_id } => {
                Self::ApplicationNotFound { application_id }
            }
            ApplicationStoreError::SystemNotFound { system_name } => {
                Self::SystemNotFound { system_name }
            }
        }
    }
}

pub fn import_application(
    connection: &mut Connection,
    repository_path: &Path,
    system_name: Option<&str>,
    repository_url: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<ApplicationSummary, ImportError> {
    let manifest_path = manifest_path.unwrap_or(DEFAULT_MANIFEST_PATH);
    let manifest = load_manifest_at(repository_path, manifest_path)
        .map_err(|source| ImportError::Manifest { source })?;

    let resolved_system_name = system_name
        .or_else(|| manifest.system.as_ref().map(|s| s.name.as_str()))
        .ok_or(ImportError::SystemRequired)?;

    let transaction = connection
        .transaction()
        .map_err(|source| ImportError::Persistence { source })?;

    if let Some(application) =
        application_store::load_application_for_import(&transaction, &manifest.application.name)?
    {
        transaction
            .commit()
            .map_err(|source| ImportError::Persistence { source })?;
        return Ok(application);
    }

    let system_id = application_store::generate_id(&transaction).map_err(ImportError::from)?;
    application_store::ensure_system(&transaction, &system_id, resolved_system_name)?;
    let system_id = application_store::load_system_id_by_name(&transaction, resolved_system_name)?;

    let application_id = application_store::generate_id(&transaction).map_err(ImportError::from)?;
    let inserted = application_store::insert_application(
        &transaction,
        &application_id,
        &system_id,
        &manifest.application.name,
        manifest.schema_version,
    )?;

    if inserted {
        persist_specification(
            &transaction,
            &application_id,
            &manifest,
            repository_url,
            manifest_path,
        )?;
    }

    let application =
        application_store::load_application_for_import(&transaction, &manifest.application.name)?
            .ok_or_else(|| ImportError::ApplicationNotFound {
            application_id: application_id.clone(),
        })?;

    transaction
        .commit()
        .map_err(|source| ImportError::Persistence { source })?;

    Ok(application)
}

fn persist_specification(
    transaction: &rusqlite::Transaction<'_>,
    application_id: &str,
    manifest: &Manifest,
    repository_url: Option<&str>,
    manifest_path: &str,
) -> Result<(), ImportError> {
    application_store::insert_delivery_spec(
        transaction,
        application_id,
        manifest.delivery.delivery_type,
        &manifest.delivery.image,
    )?;

    if let Some(repository_url) = repository_url {
        let repository_kind = if is_remote_repository(repository_url) {
            RepositoryKind::Remote
        } else {
            RepositoryKind::Local
        };

        application_store::insert_source_spec(
            transaction,
            application_id,
            repository_url,
            repository_kind,
            None,
            manifest_path,
        )?;
    }

    application_store::insert_runtime_spec(
        transaction,
        application_id,
        manifest.runtime.container_port,
    )?;
    application_store::insert_health_check_spec(
        transaction,
        application_id,
        &manifest.runtime.healthcheck_path,
        manifest.runtime.expected_status,
    )?;
    application_store::insert_exposure(
        transaction,
        application_id,
        manifest.exposure.default_visibility,
        manifest.exposure.domain.as_deref(),
    )?;

    Ok(())
}
