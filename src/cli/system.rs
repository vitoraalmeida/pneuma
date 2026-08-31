use pneuma::control::{Command, CommandResult, ControlExecutor};

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
        .map_err(CliError::from_control)?;
    let CommandResult::SystemCreated(system) = result else {
        unreachable!("SystemCreate yields SystemCreated");
    };
    println!("{}", output::created_system(&system));
    Ok(())
}

// Renders registered systems without adding CLI-layer filtering.
pub(crate) fn run_system_list(executor: &ControlExecutor, verbose: bool) -> Result<(), CliError> {
    log_verbose(verbose, "list registered systems");
    let result = executor
        .execute(Command::SystemList)
        .map_err(CliError::from_control)?;
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
        .map_err(CliError::from_control)?;
    let CommandResult::SystemDetails(details) = result else {
        unreachable!("SystemShow yields SystemDetails");
    };
    println!("{}", output::system_details(&details));
    Ok(())
}
