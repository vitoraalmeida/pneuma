use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::domain::application::Application;

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

pub fn list_applications(connection: &Connection) -> Result<Vec<Application>, ListError> {
    let mut statement = connection
        .prepare(
            "SELECT
                applications.id,
                applications.system_id,
                applications.name,
                application_sources.repository_url,
                application_sources.default_branch,
                applications.active_deployment_id
             FROM applications
             LEFT JOIN application_sources
                ON application_sources.application_id = applications.id
             ORDER BY applications.name",
        )
        .map_err(|source| ListError { source })?;
    let rows = statement
        .query_map([], |row| {
            Ok(Application {
                id: row.get(0)?,
                system_id: row.get(1)?,
                name: row.get(2)?,
                repository: row.get(3)?,
                default_branch: row.get(4)?,
                active_deployment_id: row.get(5)?,
            })
        })
        .map_err(|source| ListError { source })?;

    let mut applications = Vec::new();
    for row in rows {
        applications.push(row.map_err(|source| ListError { source })?);
    }

    Ok(applications)
}

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
