use std::error::Error;
use std::path::Path;

use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::manifest::{ManifestError, load_manifest_at};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::exposure_store::{self, ExposureStoreError};
use crate::adapters::stores::system_store;
use crate::domain::application::ApplicationSummary;
use crate::domain::git::{ApplicationSource, RelativeManifestPath};
use crate::domain::identity::ApplicationId;
use crate::domain::manifest::ImportSpecification;
use crate::domain::system::SystemName;

const DEFAULT_MANIFEST_PATH: &str = "pneuma.toml";

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("failed to import application: {source}")]
    Manifest {
        #[source]
        source: ManifestError,
    },
    #[error("failed to persist imported application: {source}")]
    Persistence {
        #[source]
        source: Box<dyn Error>,
    },
    #[error("application `{application_id}` was not found")]
    ApplicationNotFound { application_id: String },
    #[error("system is required: specify [system] in manifest or use --system flag")]
    SystemRequired,
}

impl From<ApplicationStoreError> for ImportError {
    fn from(source: ApplicationStoreError) -> Self {
        Self::Persistence {
            source: Box::new(source),
        }
    }
}

impl From<ExposureStoreError> for ImportError {
    fn from(source: ExposureStoreError) -> Self {
        Self::Persistence {
            source: Box::new(source),
        }
    }
}

impl From<rusqlite::Error> for ImportError {
    fn from(source: rusqlite::Error) -> Self {
        Self::Persistence {
            source: Box::new(source),
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
    let specification = load_manifest_at(repository_path, manifest_path)
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

    let transaction = connection.transaction()?;

    if let Some(application) =
        application_store::load_application_for_import(&transaction, &application_name)?
    {
        transaction.commit()?;
        return Ok(application);
    }

    let system = system_store::create_or_load(&transaction, &resolved_system_name, None)?;

    let application_id = application_store::generate_id(&transaction).map_err(ImportError::from)?;
    let inserted = application_store::insert_application(
        &transaction,
        &application_id,
        &system.id,
        &application_name,
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

    transaction.commit()?;

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
        specification.delivery.image_repository(),
    )?;

    if let Some(repository_url) = repository_url {
        let source = ApplicationSource::new(
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
                reason: "must be a remote Git URL without surrounding whitespace",
            },
        })?;
        application_store::insert_source_spec(transaction, application_id, &source)?;
    }

    application_store::insert_runtime_spec(
        transaction,
        application_id,
        specification.runtime.container_port(),
    )?;
    application_store::insert_health_check_spec(
        transaction,
        application_id,
        specification.runtime.health_check().path(),
        specification.runtime.health_check().expected_status(),
    )?;
    exposure_store::insert_exposure(transaction, application_id, &specification.exposure)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::{ApplicationStoreError, ExposureStoreError, ImportError, ManifestError};

    #[test]
    fn import_errors_expose_their_underlying_causes() {
        let error = ImportError::Manifest {
            source: ManifestError::UnsupportedSchemaVersion { found: 1 },
        };
        let source = error.source().expect("Manifest must keep its cause");
        assert!(matches!(
            source.downcast_ref::<ManifestError>(),
            Some(ManifestError::UnsupportedSchemaVersion { found: 1 })
        ));

        let error = ImportError::Persistence {
            source: Box::new(rusqlite::Error::InvalidParameterName("test".to_owned())),
        };
        let source = error.source().expect("Persistence must keep its cause");
        assert!(
            source
                .downcast_ref::<rusqlite::Error>()
                .is_some_and(|source| source.to_string() == "Invalid parameter name: test")
        );
    }

    #[test]
    fn store_errors_funnel_into_one_operation_level_persistence_error() {
        let error = ImportError::from(ApplicationStoreError::Persistence {
            source: rusqlite::Error::InvalidParameterName("test".to_owned()),
        });
        assert!(matches!(error, ImportError::Persistence { .. }));
        let source = error.source().expect("Persistence must keep its cause");
        assert!(
            source
                .downcast_ref::<ApplicationStoreError>()
                .is_some_and(|source| source.to_string()
                    == "application store error: Invalid parameter name: test")
        );

        let error = ImportError::from(ExposureStoreError::InvalidVisibility {
            application_id: "app-id".to_owned(),
            visibility: "unknown".to_owned(),
        });
        assert!(matches!(error, ImportError::Persistence { .. }));
        let source = error.source().expect("Persistence must keep its cause");
        assert!(source.downcast_ref::<ExposureStoreError>().is_some());
    }
}
