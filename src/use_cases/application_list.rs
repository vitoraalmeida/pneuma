use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::application_store;
use crate::domain::application::{Application, ApplicationSummary};

#[derive(Debug)]
pub struct ListError {
    source: rusqlite::Error,
}

impl fmt::Display for ListError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to list applications: {}", self.source)
    }
}

impl Error for ListError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

// Reads application summaries in display order without mutating persisted state.
pub fn list_applications(connection: &Connection) -> Result<Vec<ApplicationSummary>, ListError> {
    application_store::list_application_summaries(connection).map_err(|error| match error {
        application_store::ApplicationStoreError::Persistence { source } => ListError { source },
        _ => ListError {
            source: rusqlite::Error::QueryReturnedNoRows,
        },
    })
}

// Looks up the full application record by its operator-facing name.
pub fn find_application_by_name(
    connection: &Connection,
    name: &str,
) -> Result<Option<Application>, ListError> {
    application_store::load_application_by_name(connection, name).map_err(|error| match error {
        application_store::ApplicationStoreError::Persistence { source } => ListError { source },
        application_store::ApplicationStoreError::NotFound { .. }
        | application_store::ApplicationStoreError::SystemNotFound { .. } => ListError {
            source: rusqlite::Error::QueryReturnedNoRows,
        },
    })
}

// Determines whether an application has ever completed a successful deployment.
pub fn application_is_deployed(
    connection: &Connection,
    application_id: &str,
) -> Result<bool, ListError> {
    application_store::application_has_successful_deployment(connection, application_id).map_err(
        |error| match error {
            application_store::ApplicationStoreError::Persistence { source } => {
                ListError { source }
            }
            _ => ListError {
                source: rusqlite::Error::QueryReturnedNoRows,
            },
        },
    )
}
