use std::error::Error;
use std::fmt;

use rusqlite::Connection;

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

pub fn list_systems(connection: &Connection) -> Result<Vec<System>, ListSystemsError> {
    let mut statement = connection
        .prepare("SELECT id, name, description FROM systems ORDER BY name")
        .map_err(|source| ListSystemsError { source })?;

    let rows = statement
        .query_map([], |row| {
            Ok(System {
                id: row.get(0)?,
                name: row.get(1)?,
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
