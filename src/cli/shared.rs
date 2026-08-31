use pneuma::domain::application::Application;
use pneuma::use_cases::application::ApplicationLookupError;

use super::error::CliError;

// Re-exports the single path-configuration owner for the CLI capability modules.
pub(crate) use pneuma::config::{
    CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE, CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
    DEFAULT_CADDY_MANAGED_PATH, DEFAULT_CADDYFILE_PATH, configured_path, log_verbose,
};

// Converts use-case-typed application resolution into the CLI error vocabulary.
pub(crate) fn resolve_application(
    connection: &rusqlite::Connection,
    application_name: &str,
) -> Result<Application, CliError> {
    pneuma::use_cases::application::resolve_application(connection, application_name).map_err(
        |source| match source {
            ApplicationLookupError::NotFound { application_name } => {
                CliError::ApplicationNotFound { application_name }
            }
            ApplicationLookupError::Store { source } => CliError::ApplicationLookup { source },
        },
    )
}
