use rusqlite::Connection;

use pneuma::use_cases::application::{
    application_is_deployed, import_remote_application, list_applications,
    report_application_status, start_application, stop_application,
};

use super::error::CliError;
use super::shared::{
    DEFAULT_WORKSPACE_PATH, WORKSPACE_PATH_ENVIRONMENT_VARIABLE, configured_path, log_verbose,
    resolve_application,
};

// Clones only remote repositories into an isolated checkout, then always attempts cleanup.
pub(crate) fn run_import(
    connection: &mut Connection,
    verbose: bool,
    repository: &str,
    system_name: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("import repository: {repository}"));
    let workspace = configured_path(WORKSPACE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_WORKSPACE_PATH);
    let application = import_remote_application(
        connection,
        repository,
        &workspace,
        system_name,
        manifest_path,
    )
    .map_err(|source| CliError::Import { source });
    let application = application?;
    println!("Imported {}", application.name);
    println!("Status: Registered");
    if let Some(deployment_id) = &application.active_deployment_id {
        println!("Deployment: {deployment_id}");
    } else {
        println!("Deployment: Not deployed");
    }
    Ok(())
}

// Reports each registered application's persisted deployment state.
pub(crate) fn run_list(connection: &Connection, verbose: bool) -> Result<(), CliError> {
    log_verbose(verbose, "list registered applications");
    let applications = list_applications(connection).map_err(|source| CliError::List { source })?;
    for application in applications {
        let deployment_status = if application_is_deployed(connection, &application.id)
            .map_err(|source| CliError::List { source })?
        {
            "Deployed"
        } else {
            "Not deployed"
        };

        println!("{}\tRegistered\t{deployment_status}", application.name);
    }
    Ok(())
}

// Queries runtime status through the use case, which may inspect external runtime state.
pub(crate) fn run_status(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    log_verbose(
        verbose,
        format!("report status of application {}", application.name),
    );
    let observation = report_application_status(connection, &application.id, &application.name)
        .map_err(|source| CliError::ApplicationRuntime {
            source: Box::new(source),
        })?;
    println!("Application: {}", application.name);
    println!("Desired state: {:?}", observation.desired_runtime_state);
    println!("Observed state: {:?}", observation.observed_runtime_state);
    println!("Runtime: {}", observation.runtime_id);
    println!("Container: {}", observation.container_id);
    Ok(())
}

// Requests a runtime stop through the use case and reports its resulting observation.
pub(crate) fn run_stop(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    log_verbose(verbose, format!("stop application {}", application.name));
    let observation =
        stop_application(connection, &application.id, &application.name).map_err(|source| {
            CliError::ApplicationRuntime {
                source: Box::new(source),
            }
        })?;
    println!("Stopped {}", application.name);
    println!("Desired state: {:?}", observation.desired_runtime_state);
    println!("Observed state: {:?}", observation.observed_runtime_state);
    Ok(())
}

// Requests a runtime start through the use case and reports its resulting observation.
pub(crate) fn run_start(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    log_verbose(verbose, format!("start application {}", application.name));
    let observation =
        start_application(connection, &application.id, &application.name).map_err(|source| {
            CliError::ApplicationRuntime {
                source: Box::new(source),
            }
        })?;
    println!("Started {}", application.name);
    println!("Desired state: {:?}", observation.desired_runtime_state);
    println!("Observed state: {:?}", observation.observed_runtime_state);
    Ok(())
}
