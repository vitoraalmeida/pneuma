use std::env;

use pneuma::control::{Command, ControlExecutor};
use pneuma::use_cases::ci::{CiCommand, CiDispatchError, parse_ci_command};

use super::error::CliError;
use super::shared::log_verbose;

// Restricts SSH CI execution to the validated command carried by SSH_ORIGINAL_COMMAND.
pub(super) fn run_ci_dispatch(executor: &ControlExecutor, verbose: bool) -> Result<(), CliError> {
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
        } => super::execute_control_command(
            executor,
            Command::DeployBranch {
                application_name: application,
                branch,
            },
            verbose,
        ),
    }
}
