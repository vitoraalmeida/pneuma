use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::application_store;
use crate::adapters::stores::system_store;
use crate::domain::application::ApplicationSummary;
use crate::domain::system::System;

#[derive(Debug)]
pub enum ShowError {
    NotFound {
        system_name: String,
    },
    ApplicationStore {
        source: application_store::ApplicationStoreError,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for ShowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { system_name } => {
                write!(formatter, "system `{system_name}` was not found")
            }
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to show system: {source}")
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
            Self::ApplicationStore { source } => Some(source),
            Self::Persistence { source } => Some(source),
        }
    }
}

// Combines a System with its catalog applications for the details view.
pub struct SystemDetails {
    pub system: System,
    pub applications: Vec<ApplicationSummary>,
}

// Loads one named System and its applications without making lifecycle decisions.
pub fn show_system(connection: &Connection, system_name: &str) -> Result<SystemDetails, ShowError> {
    let system = system_store::load_by_name(connection, system_name)
        .map_err(|error| match error {
            system_store::SystemStoreError::Persistence { source } => {
                ShowError::Persistence { source }
            }
        })?
        .ok_or_else(|| ShowError::NotFound {
            system_name: system_name.to_owned(),
        })?;
    let applications =
        application_store::list_application_summaries_for_system(connection, &system.id)
            .map_err(|source| ShowError::ApplicationStore { source })?;

    Ok(SystemDetails {
        system,
        applications,
    })
}
