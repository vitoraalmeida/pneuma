use pneuma::domain::application::{Application, ApplicationName};
use pneuma::use_cases::application::find_application_by_name;

use super::error::CliError;

// Re-exports the single path-configuration owner for the CLI capability modules.
pub(crate) use pneuma::config::{
    CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE, CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
    DEFAULT_CADDY_MANAGED_PATH, DEFAULT_CADDYFILE_PATH, DEFAULT_WORKSPACE_PATH,
    WORKSPACE_PATH_ENVIRONMENT_VARIABLE, configured_path, log_verbose,
};

// Converts expected absence from the store-facing use case into a CLI-specific error.
pub(crate) fn resolve_application(
    connection: &rusqlite::Connection,
    application_name: &str,
) -> Result<Application, CliError> {
    let application_name =
        ApplicationName::new(application_name).map_err(|_| CliError::ApplicationNotFound {
            application_name: application_name.to_owned(),
        })?;
    find_application_by_name(connection, &application_name)
        .map_err(|source| CliError::ApplicationLookup { source })?
        .ok_or_else(|| CliError::ApplicationNotFound {
            application_name: application_name.as_str().to_owned(),
        })
}
