use rusqlite::Connection;

use pneuma::domain::exposure::Visibility;
use pneuma::use_cases::exposure::{ExposureChangeError, change_exposure};

use super::error::CliError;
use super::output;
use super::shared::{
    CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE, CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
    DEFAULT_CADDY_MANAGED_PATH, DEFAULT_CADDYFILE_PATH, configured_path, log_verbose,
    resolve_application,
};

// Changes visibility through the use case, which manages the Caddy side effects.
pub(crate) fn run_visibility_set(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
    visibility: Visibility,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    let managed_directory = configured_path(
        CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
        DEFAULT_CADDY_MANAGED_PATH,
    );
    let caddyfile_path =
        configured_path(CADDYFILE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_CADDYFILE_PATH);
    log_verbose(
        verbose,
        format!(
            "set visibility of application {} to {:?}",
            application.name, visibility
        ),
    );
    let exposure_change = change_exposure(
        connection,
        &application.id,
        visibility,
        &managed_directory,
        &caddyfile_path,
    )
    .map_err(|source: ExposureChangeError| CliError::VisibilitySet { source })?;
    println!(
        "{}",
        output::visibility_change(&application.name, &exposure_change)
    );
    Ok(())
}
