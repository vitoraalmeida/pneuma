use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::system_store;
use crate::domain::system::System;

#[derive(Debug)]
pub struct ListSystemsError {
    source: rusqlite::Error,
}

impl fmt::Display for ListSystemsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to list systems: {}", self.source)
    }
}

impl Error for ListSystemsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

// Lists catalog systems in stable name order without modifying persisted state.
pub fn list_systems(connection: &Connection) -> Result<Vec<System>, ListSystemsError> {
    system_store::list(connection).map_err(|error| match error {
        system_store::SystemStoreError::Persistence { source } => ListSystemsError { source },
    })
}
