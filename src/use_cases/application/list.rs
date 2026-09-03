use rusqlite::Connection;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::application::ApplicationSummary;
use crate::domain::identity::ApplicationId;

// Reads application summaries in display order without mutating persisted state.
pub fn list_applications(
    connection: &Connection,
) -> Result<Vec<ApplicationSummary>, ApplicationStoreError> {
    application_store::list_application_summaries(connection)
}

// Determines whether an application has ever completed a successful deployment.
pub fn application_is_deployed(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<bool, ApplicationStoreError> {
    application_store::application_has_successful_deployment(connection, application_id)
}

/// Catalog projection pairing one registered application with its persisted
/// deployment state for list rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationCatalogEntry {
    pub summary: ApplicationSummary,
    pub deployed: bool,
}

// Reads the catalog projection in display order without mutating persisted state.
pub fn list_application_catalog(
    connection: &Connection,
) -> Result<Vec<ApplicationCatalogEntry>, ApplicationStoreError> {
    let applications = list_applications(connection)?;
    let mut entries = Vec::with_capacity(applications.len());
    for application in applications {
        let deployed = application_is_deployed(connection, &application.id)?;
        entries.push(ApplicationCatalogEntry {
            summary: application,
            deployed,
        });
    }
    Ok(entries)
}
