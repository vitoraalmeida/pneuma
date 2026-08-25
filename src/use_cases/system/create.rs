use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::stores::system_store;
use crate::domain::system::{System, SystemName};

#[derive(Debug, Error)]
pub enum CreateError {
    #[error("failed to create system: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

// Creates a System once and returns the existing row when its name already exists.
pub fn create_system(
    connection: &mut Connection,
    name: &SystemName,
    description: Option<&str>,
) -> Result<System, CreateError> {
    let transaction = connection
        .transaction()
        .map_err(|source| CreateError::Persistence { source })?;

    let system = system_store::create_or_load(&transaction, name, description)
        .map_err(|source| CreateError::Persistence { source })?;

    transaction
        .commit()
        .map_err(|source| CreateError::Persistence { source })?;

    Ok(system)
}
