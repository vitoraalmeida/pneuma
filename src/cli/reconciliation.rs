use pneuma::control::{Command, CommandResult, ControlExecutor};

use super::error::CliError;
use super::output;
use super::shared::log_verbose;

// Reconciles persisted runtime and exposure intent through configured host integrations.
pub(crate) fn run_reconcile(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("reconcile application: {application_name}"),
    );
    let result = executor
        .execute(Command::Reconcile {
            application_name: application_name.to_owned(),
        })
        .map_err(CliError::from_control)?;
    let CommandResult::Reconciled {
        application_name,
        result,
    } = result
    else {
        unreachable!("Reconcile yields Reconciled");
    };
    println!(
        "{}",
        output::reconciliation_result(&application_name, &result)
    );
    Ok(())
}
