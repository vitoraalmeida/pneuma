use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::stores::system_store;
use crate::domain::system::System;

#[derive(Debug, Error)]
#[error("failed to list systems: {source}")]
pub struct ListSystemsError {
    #[source]
    source: rusqlite::Error,
}

// Lists catalog systems in stable name order without modifying persisted state.
pub fn list_systems(connection: &Connection) -> Result<Vec<System>, ListSystemsError> {
    system_store::list(connection).map_err(|error| match error {
        system_store::SystemStoreError::Persistence { source } => ListSystemsError { source },
    })
}
