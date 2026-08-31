use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor};

use super::error::CliError;
use super::output;
use super::shared::log_verbose;

// Adapts system creation results and errors to the CLI's output contract.
pub(crate) fn run_system_create(
    executor: &ControlExecutor,
    verbose: bool,
    name: &str,
    description: Option<&str>,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("create system: {name}"));
    let result = executor
        .execute(Command::SystemCreate {
            name: name.to_owned(),
            description: description.map(str::to_owned),
        })
        .map_err(cli_error)?;
    let CommandResult::SystemCreated(system) = result else {
        unreachable!("SystemCreate yields SystemCreated");
    };
    println!("{}", output::created_system(&system));
    Ok(())
}

// Renders registered systems without adding CLI-layer filtering.
pub(crate) fn run_system_list(executor: &ControlExecutor, verbose: bool) -> Result<(), CliError> {
    log_verbose(verbose, "list registered systems");
    let result = executor.execute(Command::SystemList).map_err(cli_error)?;
    let CommandResult::Systems(systems) = result else {
        unreachable!("SystemList yields Systems");
    };
    let rendered = output::system_list(&systems);
    if !rendered.is_empty() {
        println!("{rendered}");
    }
    Ok(())
}

// Renders the system detail view returned by the use case.
pub(crate) fn run_system_show(
    executor: &ControlExecutor,
    verbose: bool,
    name: &str,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("show system: {name}"));
    let result = executor
        .execute(Command::SystemShow {
            name: name.to_owned(),
        })
        .map_err(cli_error)?;
    let CommandResult::SystemDetails(details) = result else {
        unreachable!("SystemShow yields SystemDetails");
    };
    println!("{}", output::system_details(&details));
    Ok(())
}

// Keeps the presentation error vocabulary identical while sourcing failures from the boundary.
fn cli_error(source: ControlError) -> CliError {
    match source {
        ControlError::Database { source } => CliError::Database { source },
        ControlError::InvalidSystemName { source } => CliError::InvalidSystemName { source },
        ControlError::SystemCreate { source } => CliError::SystemCreate { source },
        ControlError::SystemList { source } => CliError::SystemList { source },
        ControlError::SystemShow { source } => CliError::SystemShow { source },
    }
}
