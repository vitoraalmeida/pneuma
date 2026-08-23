use rusqlite::Connection;

use pneuma::domain::application::ApplicationName;
use pneuma::use_cases::reconciliation::{
    ReconciliationReadError, ReconciliationResult, reconcile_application,
};

use super::error::CliError;
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
    match reconcile_application(
        connection,
        &application_name,
        &managed_caddy_directory,
        &caddyfile_path,
    )
    .map_err(|source| CliError::Reconcile { source })?
    {
        ReconciliationResult::NoOp => {
            println!("Application: {application_name}");
            println!("Result: no-op");
        }
        ReconciliationResult::Deferred {
            blocking_deployment,
        } => {
            println!("Application: {application_name}");
            println!("Result: deferred");
            if let Some(blocking_deployment) = blocking_deployment {
                println!(
                    "Blocking deployment: {} ({})",
                    blocking_deployment.id,
                    blocking_deployment.status()
                );
            }
        }
        ReconciliationResult::Repaired {
            runtime_id,
            container_id,
        } => {
            println!("Application: {application_name}");
            println!("Result: repaired");
            println!("Runtime: {runtime_id}");
            println!("Container: {container_id}");
        }
        ReconciliationResult::ManualIntervention { reason } => {
            println!("Application: {application_name}");
            println!("Result: manual-intervention");
            println!("Diagnostic: {reason}");
        }
        ReconciliationResult::ExposureRepaired => {
            println!("Application: {application_name}");
            println!("Result: repaired");
        }
        ReconciliationResult::Failed { reason } => {
            println!("Application: {application_name}");
            println!("Result: failed");
            println!("Diagnostic: {reason}");
        }
        ReconciliationResult::Diverged { reason } => {
            println!("Application: {application_name}");
            println!("Result: diverged");
            println!("Diagnostic: {reason}");
        }
    }
    Ok(())
}
