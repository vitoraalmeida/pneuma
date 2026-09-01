use pneuma::control::{Command, CommandResult, ControlExecutor};
use pneuma::domain::exposure::Visibility;

use super::error::CliError;
use super::output;
use super::shared::log_verbose;

// Changes visibility through the boundary, which manages the Caddy side effects.
pub(crate) fn run_visibility_set(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
    visibility: Visibility,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    log_verbose(
        verbose,
        format!("set visibility of application {application_name} to {visibility:?}"),
    );
    let result = executor
        .execute(Command::VisibilitySet {
            application_name: application_name.to_owned(),
            visibility,
        })
        .map_err(CliError::from_control)?;
    let CommandResult::ExposureChanged {
        application_name,
        change,
    } = result
    else {
        unreachable!("VisibilitySet yields ExposureChanged");
    };
    println!("{}", output::visibility_change(&application_name, &change));
    Ok(())
}
