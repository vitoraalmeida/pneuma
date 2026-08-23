use std::env;
use std::path::PathBuf;

use pneuma::domain::application::{Application, ApplicationName};
use pneuma::use_cases::application::find_application_by_name;

use super::error::CliError;

pub(crate) const WORKSPACE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_WORKSPACE_PATH";
pub(crate) const DEFAULT_WORKSPACE_PATH: &str = "/var/lib/pneuma/checkouts";
pub(crate) const CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_CADDY_MANAGED_PATH";
pub(crate) const DEFAULT_CADDY_MANAGED_PATH: &str = "/etc/caddy/applications";
pub(crate) const CADDYFILE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_CADDYFILE_PATH";
pub(crate) const DEFAULT_CADDYFILE_PATH: &str = "/etc/caddy/Caddyfile";

// Emits operational detail only when the global verbose flag is enabled.
pub(crate) fn log_verbose(verbose: bool, message: impl std::fmt::Display) {
    if verbose {
        eprintln!("[verbose] {message}");
    }
}

// Resolves optional path overrides consistently, treating an empty value as unset.
pub(crate) fn configured_path(variable: &str, default: &str) -> PathBuf {
    env::var_os(variable)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

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
