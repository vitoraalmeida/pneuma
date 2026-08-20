use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::domain::application::SystemName;
use crate::domain::identity::SystemId;
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
    let mut statement = connection
        .prepare("SELECT id, name, description FROM systems ORDER BY name")
        .map_err(|source| ListSystemsError { source })?;

    let rows = statement
        .query_map([], |row| {
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
        })
        .map_err(|source| ListSystemsError { source })?;

    let mut systems = Vec::new();
    for row in rows {
        systems.push(row.map_err(|source| ListSystemsError { source })?);
    }

    Ok(systems)
}
