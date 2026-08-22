use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::application::ApplicationSummary;
use crate::domain::identity::ApplicationId;

#[derive(Debug)]
pub struct ListError {
    source: ApplicationStoreError,
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
    application_store::list_application_summaries(connection).map_err(|source| ListError { source })
}

// Determines whether an application has ever completed a successful deployment.
pub fn application_is_deployed(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<bool, ListError> {
    application_store::application_has_successful_deployment(connection, application_id)
        .map_err(|source| ListError { source })
}
