use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::stores::application_store;
use crate::adapters::stores::system_store;
use crate::domain::application::ApplicationSummary;
use crate::domain::system::{System, SystemName};

#[derive(Debug, Error)]
pub enum ShowError {
    #[error("system `{system_name}` was not found")]
    NotFound { system_name: String },
    #[error("failed to show system: {source}")]
    ApplicationStore {
        #[source]
        source: application_store::ApplicationStoreError,
    },
    #[error("failed to show system: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

// Combines a System with its catalog applications for the details view.
pub struct SystemDetails {
    pub system: System,
    pub applications: Vec<ApplicationSummary>,
}

// Loads one named System and its applications without making lifecycle decisions.
pub fn show_system(
    connection: &Connection,
    system_name: &SystemName,
) -> Result<SystemDetails, ShowError> {
    let system = system_store::load_by_name(connection, system_name)
        .map_err(|source| ShowError::Persistence { source })?
        .ok_or_else(|| ShowError::NotFound {
            system_name: system_name.to_string(),
        })?;
    let applications =
        application_store::list_application_summaries_for_system(connection, &system.id)
            .map_err(|source| ShowError::ApplicationStore { source })?;

    Ok(SystemDetails {
        system,
        applications,
    })
}
