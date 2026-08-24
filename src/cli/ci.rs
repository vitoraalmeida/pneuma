use std::env;

use pneuma::adapters::database;
use pneuma::use_cases::ci::{CiCommand, CiDispatchError, parse_ci_command};

use super::error::CliError;
use super::shared::log_verbose;

// Restricts SSH CI execution to the validated command carried by SSH_ORIGINAL_COMMAND.
pub(crate) fn run_ci_dispatch(verbose: bool) -> Result<(), CliError> {
    let original_command = env::var("SSH_ORIGINAL_COMMAND").map_err(|_| CliError::CiDispatch {
        source: CiDispatchError::MissingSshOriginalCommand,
    })?;

    log_verbose(verbose, format!("CI command: {original_command}"));

    let ci_command =
        parse_ci_command(&original_command).map_err(|source| CliError::CiDispatch { source })?;

    match ci_command {
        CiCommand::Version => {
            super::run_version();
            Ok(())
        }
        CiCommand::Deploy {
            application,
            branch,
        } => {
            let database_path = database::configured_path();

            let mut connection =
                database::open(&database_path).map_err(|source| CliError::Database { source })?;

            super::deployment::run_deploy_branch(&mut connection, verbose, &application, &branch)
        }
    }
}
