use rusqlite::Connection;

use pneuma::domain::application::ApplicationName;
use pneuma::use_cases::reconciliation::{ReconciliationReadError, reconcile_application};

use super::error::CliError;
use super::output;
use super::shared::{
    CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE, CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
    DEFAULT_CADDY_MANAGED_PATH, DEFAULT_CADDYFILE_PATH, configured_path, log_verbose,
};

// Reconciles persisted runtime and exposure intent through configured host integrations.
pub(crate) fn run_reconcile(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("reconcile application: {application_name}"),
    );
    let managed_caddy_directory = configured_path(
        CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
        DEFAULT_CADDY_MANAGED_PATH,
    );
    let caddyfile_path =
        configured_path(CADDYFILE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_CADDYFILE_PATH);
    let application_name =
        ApplicationName::new(application_name).map_err(|_| CliError::Reconcile {
            source: ReconciliationReadError::ApplicationNotFound {
                application_name: application_name.to_owned(),
            },
        })?;
    let result = reconcile_application(
        connection,
        &application_name,
        &managed_caddy_directory,
        &caddyfile_path,
    )
    .map_err(|source| CliError::Reconcile { source })?;
    println!(
        "{}",
        output::reconciliation_result(&application_name, &result)
    );
    Ok(())
}
