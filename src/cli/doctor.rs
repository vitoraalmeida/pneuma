use std::path::Path;

use rusqlite::Connection;

use pneuma::adapters::database::{self, DatabaseError};
use pneuma::adapters::diagnostics;

use super::error::CliError;
use super::output;

// Runs diagnostic checks without failing on a missing database connection.
pub(crate) fn run_doctor(connection: &Connection, verbose: bool) -> Result<(), CliError> {
    if diagnostics::run(connection, verbose) {
        Ok(())
    } else {
        Err(CliError::Doctor)
    }
}

// Opens the database for the doctor command, reporting the failure as diagnostic output.
pub(crate) fn open_doctor_connection(database_path: &Path) -> Result<Connection, CliError> {
    database::open(database_path).map_err(|source| {
        println!("{}", output::doctor_connection_failure(database_path));
        CliError::Database { source }
    })
}

// Copies the live database to the requested backup path.
pub(crate) fn run_database_backup(database_path: &Path, path: &Path) -> Result<(), CliError> {
    database::backup(database_path, path).map_err(|source| CliError::Database { source })?;
    println!("Database backup: {}", path.display());
    Ok(())
}

// Restores the database from the requested path after verifying it.
pub(crate) fn run_database_restore(database_path: &Path, path: &Path) -> Result<(), CliError> {
    let pre_restore = database::restore_and_verify(database_path, path)
        .map_err(|source: DatabaseError| CliError::Database { source })?;
    println!("Database restored from {}", path.display());
    println!("Pre-restore backup: {}", pre_restore.display());
    Ok(())
}
