use std::error::Error;
use std::fmt;
use std::path::Path;

use rusqlite::Connection;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::exposure_store::{self, ExposureStoreError};
use crate::adapters::stores::system_store::{self, SystemStoreError};
use crate::domain::application::ApplicationSummary;
use crate::domain::git::{ApplicationSource, RelativeManifestPath};
use crate::domain::identity::ApplicationId;
use crate::domain::manifest::{ImportSpecification, ManifestError, load_manifest_at};
use crate::domain::system::SystemName;

const DEFAULT_MANIFEST_PATH: &str = "pneuma.toml";

#[derive(Debug)]
pub enum ImportError {
    Manifest { source: ManifestError },
    Persistence { source: rusqlite::Error },
    ApplicationStore { source: ApplicationStoreError },
    ExposureStore { source: ExposureStoreError },
    ApplicationNotFound { application_id: String },
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
            Self::ApplicationStore { source } => {
                write!(
                    formatter,
                    "failed to persist imported application: {source}"
                )
            }
            Self::ExposureStore { source } => {
                write!(
                    formatter,
                    "failed to persist imported application: {source}"
                )
            }
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
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
            Self::ApplicationStore { source } => Some(source),
            Self::ExposureStore { source } => Some(source),
            Self::ApplicationNotFound { .. } | Self::SystemRequired => None,
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
            ApplicationStoreError::InvalidDesiredRuntimeState { .. } => {
                Self::ApplicationStore { source: error }
            }
        }
    }
}

impl From<ExposureStoreError> for ImportError {
    fn from(error: ExposureStoreError) -> Self {
        match error {
            ExposureStoreError::Persistence { source } => Self::Persistence { source },
            error => Self::ExposureStore { source: error },
        }
    }
}

impl From<SystemStoreError> for ImportError {
    fn from(error: SystemStoreError) -> Self {
        match error {
            SystemStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

// Imports a manifest and atomically creates its application specification, returning an
// existing application unchanged so repeated imports remain idempotent.
pub fn import_application(
    connection: &mut Connection,
    repository_path: &Path,
    system_name: Option<&SystemName>,
    repository_url: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<ApplicationSummary, ImportError> {
    let manifest_path = manifest_path.unwrap_or(DEFAULT_MANIFEST_PATH);
    let manifest = load_manifest_at(repository_path, manifest_path)
        .map_err(|source| ImportError::Manifest { source })?;
    let specification = manifest
        .import_specification()
        .map_err(|source| ImportError::Manifest { source })?;

    let resolved_system_name = match system_name {
        Some(system_name) => system_name.clone(),
        None => {
            let name = specification
                .system_name
                .as_ref()
                .ok_or(ImportError::SystemRequired)?;
            name.clone()
        }
    };
    let application_name = specification.application_name.clone();

    let transaction = connection
        .transaction()
        .map_err(|source| ImportError::Persistence { source })?;

    if let Some(application) =
        application_store::load_application_for_import(&transaction, &application_name)?
    {
        transaction
            .commit()
            .map_err(|source| ImportError::Persistence { source })?;
        return Ok(application);
    }

    let system = system_store::create_or_load(&transaction, &resolved_system_name, None)?;

    let application_id = application_store::generate_id(&transaction).map_err(ImportError::from)?;
    let inserted = application_store::insert_application(
        &transaction,
        &application_id,
        &system.id,
        &application_name,
        specification.schema_version,
    )?;

    if inserted {
        persist_specification(
            &transaction,
            &application_id,
            &specification,
            repository_url,
            manifest_path,
        )?;
    }

    let application =
        application_store::load_application_for_import(&transaction, &application_name)?
            .ok_or_else(|| ImportError::ApplicationNotFound {
                application_id: application_id.to_string(),
            })?;

    transaction
        .commit()
        .map_err(|source| ImportError::Persistence { source })?;

    Ok(application)
}

// Persists every manifest-derived specification within the caller's transaction so no
// partially imported application can become visible.
fn persist_specification(
    transaction: &rusqlite::Transaction<'_>,
    application_id: &ApplicationId,
    specification: &ImportSpecification,
    repository_url: Option<&str>,
    manifest_path: &str,
) -> Result<(), ImportError> {
    application_store::insert_delivery_spec(
        transaction,
        application_id,
        specification.delivery_type,
        &specification.repository,
    )?;

    if let Some(repository_url) = repository_url {
        let source = ApplicationSource::from_location(
            repository_url,
            None,
            RelativeManifestPath::new(manifest_path).map_err(|_| ImportError::Manifest {
                source: ManifestError::InvalidField {
                    field: "manifest_path",
                    reason: "must be a relative path within the checkout",
                },
            })?,
        )
        .map_err(|_| ImportError::Manifest {
            source: ManifestError::InvalidField {
                field: "repository",
                reason: "must not be empty or contain surrounding whitespace",
            },
        })?;
        application_store::insert_source_spec(transaction, application_id, &source)?;
    }

    application_store::insert_runtime_spec(
        transaction,
        application_id,
        specification.container_port,
    )?;
    application_store::insert_health_check_spec(
        transaction,
        application_id,
        &specification.healthcheck_path,
        specification.expected_status,
    )?;
    exposure_store::insert_exposure(transaction, application_id, &specification.exposure)?;

    Ok(())
}
