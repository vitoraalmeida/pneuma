use pneuma::control::{Command, CommandResult, ControlExecutor};
use pneuma::use_cases::application::{
    report_application_status, start_application, stop_application,
};

use super::error::CliError;
use super::output;
use super::shared::{log_verbose, resolve_application};

// Imports a remote repository through the boundary and renders its summary.
pub(crate) fn run_import(
    executor: &ControlExecutor,
    verbose: bool,
    repository: &str,
    system_name: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("import repository: {repository}"));
    let result = executor
        .execute(Command::ImportApplication {
            repository: repository.to_owned(),
            system_name: system_name.map(str::to_owned),
            manifest_path: manifest_path.map(str::to_owned),
        })
        .map_err(CliError::from_control)?;
    let CommandResult::ApplicationImported(application) = result else {
        unreachable!("ImportApplication yields ApplicationImported");
    };
    println!("{}", output::imported_application(&application));
    Ok(())
}

// Renders the catalog projection returned by the boundary.
pub(crate) fn run_list(executor: &ControlExecutor, verbose: bool) -> Result<(), CliError> {
    log_verbose(verbose, "list registered applications");
    let result = executor
        .execute(Command::ListApplications)
        .map_err(CliError::from_control)?;
    let CommandResult::Applications(entries) = result else {
        unreachable!("ListApplications yields Applications");
    };
    let rendered = output::application_list(&entries).trim_end().to_owned();
    if !rendered.is_empty() {
        println!("{rendered}");
    }
    Ok(())
}

// Queries runtime status through the use case, which may inspect external runtime state.
pub(crate) fn run_status(
    connection: &mut rusqlite::Connection,
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
    println!("{}", output::runtime_status(&observation));
    Ok(())
}

// Requests a runtime stop through the use case and reports its resulting observation.
pub(crate) fn run_stop(
    connection: &mut rusqlite::Connection,
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
    println!("{}", output::lifecycle_outcome(&observation));
    Ok(())
}

// Requests a runtime start through the use case and reports its resulting observation.
pub(crate) fn run_start(
    connection: &mut rusqlite::Connection,
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
    println!("{}", output::lifecycle_outcome(&observation));
    Ok(())
}
