use std::error::Error;
use std::fmt;

use rusqlite::{Connection, params};

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

pub fn create_system(
    connection: &mut Connection,
    name: &str,
    description: Option<&str>,
) -> Result<System, CreateError> {
    let transaction = connection
        .transaction()
        .map_err(|source| CreateError::Persistence { source })?;

    let system_id = transaction
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|source| CreateError::Persistence { source })?;

    transaction
        .execute(
            "INSERT INTO systems (id, name, description, created_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
             ON CONFLICT(name) DO NOTHING",
            params![system_id, name, description],
        )
        .map_err(|source| CreateError::Persistence { source })?;

    let system = transaction
        .query_row(
            "SELECT id, name, description FROM systems WHERE name = ?1",
            [name],
            |row| {
                Ok(System {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                })
            },
        )
        .map_err(|source| CreateError::Persistence { source })?;

    transaction
        .commit()
        .map_err(|source| CreateError::Persistence { source })?;

    Ok(system)
}
