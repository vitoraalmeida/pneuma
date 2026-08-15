use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::application_store;
use crate::domain::application::{Application, ApplicationSummary};

#[derive(Debug)]
pub struct ListError {
    source: rusqlite::Error,
}

impl fmt::Display for ListError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to list applications: {}", self.source)
    }
}

impl Error for ListError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

// Reads application summaries in display order without mutating persisted state.
pub fn list_applications(connection: &Connection) -> Result<Vec<ApplicationSummary>, ListError> {
    let mut statement = connection
        .prepare(
            "SELECT
                applications.id,
                applications.system_id,
                applications.name,
                applications.desired_runtime_state,
                applications.active_deployment_id,
                applications.spec_version,
                application_sources.repository_url,
                application_sources.default_branch
             FROM applications
             LEFT JOIN application_sources
                ON application_sources.application_id = applications.id
             ORDER BY applications.name",
        )
        .map_err(|source| ListError { source })?;
    let rows = statement
        .query_map([], application_store::map_application_summary_row)
        .map_err(|source| ListError { source })?;

    let mut applications = Vec::new();
    for row in rows {
        applications.push(row.map_err(|source| ListError { source })?);
    }

    Ok(applications)
}

// Looks up the full application record by its operator-facing name.
pub fn find_application_by_name(
    connection: &Connection,
    name: &str,
) -> Result<Option<Application>, ListError> {
    application_store::load_application_by_name(connection, name).map_err(|error| match error {
        application_store::ApplicationStoreError::Persistence { source } => ListError { source },
        application_store::ApplicationStoreError::NotFound { .. }
        | application_store::ApplicationStoreError::SystemNotFound { .. } => ListError {
            source: rusqlite::Error::QueryReturnedNoRows,
        },
    })
}

// Determines whether an application has ever completed a successful deployment.
pub fn application_is_deployed(
    connection: &Connection,
    application_id: &str,
) -> Result<bool, ListError> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM deployments
                WHERE application_id = ?1
                AND status = 'succeeded'
            )",
            [application_id],
            |row| row.get(0),
        )
        .map_err(|source| ListError { source })
}
