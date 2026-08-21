use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::domain::identity::SystemId;
use crate::domain::system::System;
use crate::domain::system::SystemName;

#[derive(Debug)]
pub enum SystemStoreError {
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for SystemStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persistence { source } => write!(formatter, "system store error: {source}"),
        }
    }
}
impl Error for SystemStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
        }
    }
}

pub fn create_or_load(
    transaction: &Transaction<'_>,
    name: &str,
    description: Option<&str>,
) -> Result<System, SystemStoreError> {
    let id: String = transaction
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(persistence)?;
    transaction.execute("INSERT INTO systems (id, name, description, created_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP) ON CONFLICT(name) DO NOTHING", params![id, name, description]).map_err(persistence)?;
    transaction
        .query_row(
            "SELECT id, name, description FROM systems WHERE name = ?1",
            [name],
            map_system,
        )
        .map_err(persistence)
}

pub fn list(connection: &Connection) -> Result<Vec<System>, SystemStoreError> {
    let mut statement = connection
        .prepare("SELECT id, name, description FROM systems ORDER BY name")
        .map_err(persistence)?;
    statement
        .query_map([], map_system)
        .map_err(persistence)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(persistence)
}

pub fn load_by_name(
    connection: &Connection,
    name: &str,
) -> Result<Option<System>, SystemStoreError> {
    connection
        .query_row(
            "SELECT id, name, description FROM systems WHERE name = ?1",
            [name],
            map_system,
        )
        .optional()
        .map_err(persistence)
}

fn map_system(row: &rusqlite::Row<'_>) -> rusqlite::Result<System> {
    Ok(System {
        id: SystemId::from(row.get::<_, String>(0)?),
        name: SystemName::new(&row.get::<_, String>(1)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        description: row.get(2)?,
    })
}
fn persistence(source: rusqlite::Error) -> SystemStoreError {
    SystemStoreError::Persistence { source }
}
