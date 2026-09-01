use pneuma::control::{Command, CommandResult, ControlExecutor};

use super::error::CliError;
use super::output;
use super::shared::log_verbose;

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

// Queries runtime status through the boundary and renders its observation.
pub(crate) fn run_status(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    log_verbose(
        verbose,
        format!("report status of application {application_name}"),
    );
    let result = executor
        .execute(Command::ApplicationStatus {
            application_name: application_name.to_owned(),
        })
        .map_err(CliError::from_control)?;
    let CommandResult::ApplicationStatus {
        application_name,
        observation,
    } = result
    else {
        unreachable!("ApplicationStatus yields ApplicationStatus");
    };
    println!("Application: {application_name}");
    println!("{}", output::runtime_status(&observation));
    Ok(())
}

// Requests a runtime stop through the boundary and renders its resulting observation.
pub(crate) fn run_stop(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    log_verbose(verbose, format!("stop application {application_name}"));
    let result = executor
        .execute(Command::ApplicationStop {
            application_name: application_name.to_owned(),
        })
        .map_err(CliError::from_control)?;
    let CommandResult::ApplicationStopped {
        application_name,
        observation,
    } = result
    else {
        unreachable!("ApplicationStop yields ApplicationStopped");
    };
    println!("Stopped {application_name}");
    println!("{}", output::lifecycle_outcome(&observation));
    Ok(())
}

// Requests a runtime start through the boundary and renders its resulting observation.
pub(crate) fn run_start(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    log_verbose(verbose, format!("start application {application_name}"));
    let result = executor
        .execute(Command::ApplicationStart {
            application_name: application_name.to_owned(),
        })
        .map_err(CliError::from_control)?;
    let CommandResult::ApplicationStarted {
        application_name,
        observation,
    } = result
    else {
        unreachable!("ApplicationStart yields ApplicationStarted");
    };
    println!("Started {application_name}");
    println!("{}", output::lifecycle_outcome(&observation));
    Ok(())
}
