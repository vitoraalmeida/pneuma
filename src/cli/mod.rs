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

use pneuma::adapters::database;

use error::CliError;
use shared::log_verbose;

pub(crate) use args::{Command, Invocation, parse_invocation};

// Routes commands so diagnostics, backup, restore, and SSH CI avoid unnecessary database work.
pub(crate) fn run(invocation: Invocation) -> Result<(), CliError> {
    let Invocation { verbose, command } = invocation;

    if matches!(
        command,
        Command::Version
            | Command::Doctor
            | Command::DatabaseBackup { .. }
            | Command::DatabaseRestore { .. }
    ) {
        let database_path = database::configured_path();

        if matches!(command, Command::Version) {
            run_version();
            return Ok(());
        }
        if let Command::DatabaseBackup { path } = command {
            return doctor::run_database_backup(&database_path, &path);
        }
        if let Command::DatabaseRestore { path } = command {
            return doctor::run_database_restore(&database_path, &path);
        }

        let connection = doctor::open_doctor_connection(&database_path)?;
        return doctor::run_doctor(&connection, verbose);
    }

    if matches!(command, Command::CiDispatch) {
        return ci::run_ci_dispatch(verbose);
    }

    let database_path = database::configured_path();
    log_verbose(verbose, format!("database: {}", database_path.display()));
    let mut connection =
        database::open(&database_path).map_err(|source| CliError::Database { source })?;

    match command {
        Command::SystemCreate { name, description } => {
            system::run_system_create(&mut connection, verbose, &name, description.as_deref())
        }
        Command::SystemList => system::run_system_list(&connection, verbose),
        Command::SystemShow { name } => system::run_system_show(&connection, verbose, &name),
        Command::Import {
            repository,
            system_name,
            manifest_path,
        } => application::run_import(
            &mut connection,
            verbose,
            &repository,
            system_name.as_deref(),
            manifest_path.as_deref(),
        ),
        Command::List => application::run_list(&connection, verbose),
        Command::Deployments { application_name } => {
            deployment::run_deployments(&connection, verbose, &application_name)
        }
        Command::Status { application_name } => {
            application::run_status(&mut connection, verbose, &application_name)
        }
        Command::Stop { application_name } => {
            application::run_stop(&mut connection, verbose, &application_name)
        }
        Command::Start { application_name } => {
            application::run_start(&mut connection, verbose, &application_name)
        }
        Command::Deploy {
            application_name,
            image_reference,
            branch,
        } => deployment::run_deploy(
            &mut connection,
            verbose,
            &application_name,
            image_reference,
            branch,
        ),
        Command::Rollback { application_name } => {
            deployment::run_rollback(&mut connection, verbose, &application_name)
        }
        Command::VisibilitySet {
            application_name,
            visibility,
        } => exposure::run_visibility_set(&mut connection, verbose, &application_name, visibility),
        Command::Reconcile { application_name } => {
            reconciliation::run_reconcile(&mut connection, verbose, &application_name)
        }
        Command::Doctor
        | Command::Version
        | Command::DatabaseBackup { .. }
        | Command::DatabaseRestore { .. }
        | Command::CiDispatch => unreachable!(),
    }
}

// Prints version information without requiring host configuration or database access.
pub(crate) fn run_version() {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
}
