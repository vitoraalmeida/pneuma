use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::application::{Application, ApplicationName};

#[derive(Debug, Error)]
#[error("failed to list applications: {source}")]
pub struct LookupError {
    #[source]
    source: ApplicationStoreError,
}

// Looks up the full application record by its operator-facing name.
pub fn find_application_by_name(
    connection: &Connection,
    name: &ApplicationName,
) -> Result<Option<Application>, LookupError> {
    application_store::load_application_by_name(connection, name)
        .map_err(|source| LookupError { source })
}
