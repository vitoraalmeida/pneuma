use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::application::Application;

#[derive(Debug)]
pub struct LookupError {
    source: ApplicationStoreError,
}

impl fmt::Display for LookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to list applications: {}", self.source)
    }
}

impl Error for LookupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

// Looks up the full application record by its operator-facing name.
pub fn find_application_by_name(
    connection: &Connection,
    name: &str,
) -> Result<Option<Application>, LookupError> {
    application_store::load_application_by_name(connection, name)
        .map_err(|source| LookupError { source })
}
