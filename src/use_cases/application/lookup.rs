use rusqlite::Connection;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::application::{Application, ApplicationName};

// Looks up the full application record by its operator-facing name.
pub fn find_application_by_name(
    connection: &Connection,
    name: &ApplicationName,
) -> Result<Option<Application>, ApplicationStoreError> {
    application_store::load_application_by_name(connection, name)
}
