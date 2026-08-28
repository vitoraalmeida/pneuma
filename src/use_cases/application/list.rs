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
