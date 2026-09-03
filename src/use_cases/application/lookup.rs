use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::application::{Application, ApplicationName};

// Looks up the full application record by its operator-facing name.
pub fn find_application_by_name(
    connection: &Connection,
    name: &ApplicationName,
) -> Result<Option<Application>, ApplicationStoreError> {
    application_store::load_application_by_name(connection, name)
}

#[derive(Debug, Error)]
pub enum ApplicationLookupError {
    #[error("application `{application_name}` was not found")]
    NotFound { application_name: String },
    #[error("failed to load application: {source}")]
    Store {
        #[source]
        source: ApplicationStoreError,
    },
}

// Resolves one application by its operator-facing name, including validation,
// so every caller shares the same expected-absence semantics.
pub fn resolve_application(
    connection: &Connection,
    application_name: &str,
) -> Result<Application, ApplicationLookupError> {
    let name =
        ApplicationName::new(application_name).map_err(|_| ApplicationLookupError::NotFound {
            application_name: application_name.to_owned(),
        })?;
    find_application_by_name(connection, &name)
        .map_err(|source| ApplicationLookupError::Store { source })?
        .ok_or_else(|| ApplicationLookupError::NotFound {
            application_name: name.as_str().to_owned(),
        })
}
