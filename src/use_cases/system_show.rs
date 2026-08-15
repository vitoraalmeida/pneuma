use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::application_store;
use crate::domain::application::Application;
use crate::domain::system::System;

#[derive(Debug)]
pub enum ShowError {
    NotFound { system_name: String },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for ShowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { system_name } => {
                write!(formatter, "system `{system_name}` was not found")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to show system: {source}")
            }
        }
    }
}

impl Error for ShowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NotFound { .. } => None,
            Self::Persistence { source } => Some(source),
        }
    }
}

pub struct SystemDetails {
    pub system: System,
    pub applications: Vec<Application>,
}

pub fn show_system(connection: &Connection, system_name: &str) -> Result<SystemDetails, ShowError> {
    let system = connection
        .query_row(
            "SELECT id, name, description FROM systems WHERE name = ?1",
            [system_name],
            |row| {
                Ok(System {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                })
            },
        )
        .map_err(|source| match source {
            rusqlite::Error::QueryReturnedNoRows => ShowError::NotFound {
                system_name: system_name.to_owned(),
            },
            other => ShowError::Persistence { source: other },
        })?;

    let mut statement = connection
        .prepare(
            "SELECT
                applications.id,
                applications.system_id,
                applications.name,
                application_sources.repository_url,
                application_sources.default_branch,
                applications.desired_runtime_state,
                applications.active_deployment_id,
                applications.spec_version
             FROM applications
             LEFT JOIN application_sources
                ON application_sources.application_id = applications.id
             WHERE applications.system_id = ?1
             ORDER BY applications.name",
        )
        .map_err(|source| ShowError::Persistence { source })?;

    let rows = statement
        .query_map([&system.id], application_store::map_application_row)
        .map_err(|source| ShowError::Persistence { source })?;

    let mut applications = Vec::new();
    for row in rows {
        applications.push(row.map_err(|source| ShowError::Persistence { source })?);
    }

    Ok(SystemDetails {
        system,
        applications,
    })
}
