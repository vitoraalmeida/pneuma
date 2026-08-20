use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::system_store;
use crate::domain::system::System;

#[derive(Debug)]
pub enum CreateError {
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for CreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persistence { source } => {
                write!(formatter, "failed to create system: {source}")
            }
        }
    }
}

impl Error for CreateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
        }
    }
}

// Creates a System once and returns the existing row when its name already exists.
pub fn create_system(
    connection: &mut Connection,
    name: &str,
    description: Option<&str>,
) -> Result<System, CreateError> {
    let transaction = connection
        .transaction()
        .map_err(|source| CreateError::Persistence { source })?;

    let system = system_store::create_or_load(&transaction, name, description).map_err(
        |error| match error {
            system_store::SystemStoreError::Persistence { source } => {
                CreateError::Persistence { source }
            }
        },
    )?;

    transaction
        .commit()
        .map_err(|source| CreateError::Persistence { source })?;

    Ok(system)
}
