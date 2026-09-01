mod application;
mod args;
mod ci;
mod deployment;
mod doctor;
mod error;
mod exposure;
mod output;
mod reconciliation;
mod shared;
mod system;

use pneuma::control::ControlExecutor;

use error::CliError;
use shared::log_verbose;

pub(crate) use args::{Command, Invocation, parse_invocation};

// Routes parsed commands into the interface-neutral control boundary.
pub(crate) fn run(invocation: Invocation) -> Result<(), CliError> {
    let Invocation { verbose, command } = invocation;

    if matches!(command, Command::Version) {
        run_version();
        return Ok(());
    }

    if matches!(command, Command::CiDispatch) {
        return ci::run_ci_dispatch(&ControlExecutor::from_environment(), verbose);
    }

    let executor = ControlExecutor::from_environment();
    if !matches!(
        command,
        Command::Doctor | Command::DatabaseBackup { .. } | Command::DatabaseRestore { .. }
    ) {
        log_verbose(
            verbose,
            format!("database: {}", executor.host().database_path.display()),
        );
    }

    match command {
        Command::SystemCreate { name, description } => {
            system::run_system_create(&executor, verbose, &name, description.as_deref())
        }
        Command::SystemList => system::run_system_list(&executor, verbose),
        Command::SystemShow { name } => system::run_system_show(&executor, verbose, &name),
        Command::Import {
            repository,
            system_name,
            manifest_path,
        } => application::run_import(
            &executor,
            verbose,
            &repository,
            system_name.as_deref(),
            manifest_path.as_deref(),
        ),
        Command::List => application::run_list(&executor, verbose),
        Command::Deployments { application_name } => {
            deployment::run_deployments(&executor, verbose, &application_name)
        }
        Command::Status { application_name } => {
            application::run_status(&executor, verbose, &application_name)
        }
        Command::Stop { application_name } => {
            application::run_stop(&executor, verbose, &application_name)
        }
        Command::Start { application_name } => {
            application::run_start(&executor, verbose, &application_name)
        }
        Command::VisibilitySet {
            application_name,
            visibility,
        } => exposure::run_visibility_set(&executor, verbose, &application_name, visibility),
        Command::Reconcile { application_name } => {
            reconciliation::run_reconcile(&executor, verbose, &application_name)
        }
        Command::Deploy {
            application_name,
            image_reference,
            branch,
        } => deployment::run_deploy(
            &executor,
            verbose,
            &application_name,
            image_reference,
            branch,
        ),
        Command::Rollback { application_name } => {
            deployment::run_rollback(&executor, verbose, &application_name)
        }
        Command::Doctor => doctor::run_doctor(&executor, verbose),
        Command::DatabaseBackup { path } => doctor::run_database_backup(&executor, &path),
        Command::DatabaseRestore { path } => doctor::run_database_restore(&executor, &path),
        Command::CiDispatch | Command::Version => unreachable!(),
    }
}

// Prints version information without requiring host configuration or database access.
pub(crate) fn run_version() {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
}
