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

use std::path::Path;

use pneuma::adapters::database::{self, DatabaseError, DatabaseLock, LockMode};
use pneuma::control::ControlExecutor;

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
        if let Command::DatabaseRestore { path } = command {
            // Restore takes the exclusive database-wide lock itself.
            return doctor::run_database_restore(&database_path, &path);
        }

        let _lock = shared_database_lock(&database_path)?;
        if let Command::DatabaseBackup { path } = command {
            return doctor::run_database_backup(&database_path, &path);
        }

        let connection = doctor::open_doctor_connection(&database_path)?;
        return doctor::run_doctor(&connection, verbose);
    }

    if matches!(command, Command::CiDispatch) {
        return ci::run_ci_dispatch(verbose);
    }

    let database_path = database::configured_path();
    log_verbose(verbose, format!("database: {}", database_path.display()));
    let executor = ControlExecutor::from_environment();

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
        other => {
            let _lock = shared_database_lock(&database_path)?;
            let mut connection =
                database::open(&database_path).map_err(|source| CliError::Database { source })?;

            match other {
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
                } => exposure::run_visibility_set(
                    &mut connection,
                    verbose,
                    &application_name,
                    visibility,
                ),
                Command::Reconcile { application_name } => {
                    reconciliation::run_reconcile(&mut connection, verbose, &application_name)
                }
                Command::SystemCreate { .. }
                | Command::SystemList
                | Command::SystemShow { .. }
                | Command::Import { .. }
                | Command::List
                | Command::Deployments { .. }
                | Command::Doctor
                | Command::Version
                | Command::DatabaseBackup { .. }
                | Command::DatabaseRestore { .. }
                | Command::CiDispatch => unreachable!(),
            }
        }
    }
}

// Acquires the shared database-wide lock held for as long as the command uses the database.
fn shared_database_lock(database_path: &Path) -> Result<DatabaseLock, CliError> {
    match DatabaseLock::try_acquire(database_path, LockMode::Shared) {
        Ok(Some(lock)) => Ok(lock),
        Ok(None) => Err(CliError::Database {
            source: DatabaseError::DatabaseBusy {
                path: database_path.to_path_buf(),
            },
        }),
        Err(source) => Err(CliError::Database { source }),
    }
}

// Prints version information without requiring host configuration or database access.
pub(crate) fn run_version() {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
}
