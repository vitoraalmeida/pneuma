use pneuma::control::{Command, ControlExecutor};

use super::error::CliError;
use super::progress::DeploymentProgressRenderer;
use super::shared::log_verbose;

// Deploys a supplied OCI reference through the interface-neutral boundary.
pub(crate) fn run_deploy_oci(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
    image_reference: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let mut renderer = DeploymentProgressRenderer::new(verbose, Some(("image", image_reference)));
    let mut events = |event| renderer.report(event);
    let result = executor.execute_with_events(
        Command::DeployImage {
            application_name: application_name.to_owned(),
            image_reference: image_reference.to_owned(),
        },
        &mut events,
    );
    renderer.finish();
    super::render_command_result(result.map_err(CliError::from_control)?, verbose)
}

// Resolves and deploys the requested branch through the interface-neutral boundary.
pub(crate) fn run_deploy_branch(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
    branch: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let mut renderer = DeploymentProgressRenderer::new(verbose, Some(("branch", branch)));
    let mut events = |event| renderer.report(event);
    let result = executor.execute_with_events(
        Command::DeployBranch {
            application_name: application_name.to_owned(),
            branch: branch.to_owned(),
        },
        &mut events,
    );
    renderer.finish();
    super::render_command_result(result.map_err(CliError::from_control)?, verbose)
}

// Rolls back through the interface-neutral boundary.
pub(crate) fn run_rollback(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let mut renderer = DeploymentProgressRenderer::new(verbose, None);
    let mut events = |event| renderer.report(event);
    let result = executor.execute_with_events(
        Command::Rollback {
            application_name: application_name.to_owned(),
        },
        &mut events,
    );
    renderer.finish();
    super::render_command_result(result.map_err(CliError::from_control)?, verbose)
}
