use rusqlite::Connection;

use pneuma::domain::system::SystemName;
use pneuma::use_cases::system::{create_system, list_systems, show_system};

use super::error::CliError;
use super::shared::log_verbose;

// Adapts system creation results and errors to the CLI's output contract.
pub(crate) fn run_system_create(
    connection: &mut Connection,
    verbose: bool,
    name: &str,
    description: Option<&str>,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("create system: {name}"));
    let name = SystemName::new(name).map_err(|source| CliError::InvalidSystemName { source })?;
    let system = create_system(connection, &name, description)
        .map_err(|source| CliError::SystemCreate { source })?;
    println!("Created {}", system.name);
    Ok(())
}

// Renders registered systems without adding CLI-layer filtering.
pub(crate) fn run_system_list(connection: &Connection, verbose: bool) -> Result<(), CliError> {
    log_verbose(verbose, "list registered systems");
    let systems = list_systems(connection).map_err(|source| CliError::SystemList { source })?;
    for system in systems {
        println!("{}", system.name);
    }
    Ok(())
}

// Renders the system detail view returned by the use case.
pub(crate) fn run_system_show(
    connection: &Connection,
    verbose: bool,
    name: &str,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("show system: {name}"));
    let name = SystemName::new(name).map_err(|source| CliError::InvalidSystemName { source })?;
    let details =
        show_system(connection, &name).map_err(|source| CliError::SystemShow { source })?;
    println!("System: {}", details.system.name);
    if let Some(description) = &details.system.description {
        println!("Description: {description}");
    }
    if details.applications.is_empty() {
        println!("Applications: (none)");
    } else {
        println!("Applications:");
        for application in &details.applications {
            println!("  {}", application.name);
        }
    }
    Ok(())
}
