use pneuma::control::{Command, CommandResult, ControlExecutor};

use super::error::CliError;
use super::output;
use super::progress::DeploymentProgressRenderer;
use super::shared::log_verbose;

// Lists the deployment history resolved by the boundary.
pub(crate) fn run_deployments(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let result = executor
        .execute(Command::ListDeployments {
            application_name: application_name.to_owned(),
        })
        .map_err(CliError::from_control)?;
    let CommandResult::ApplicationDeployments {
        application_name,
        deployments,
    } = result
    else {
        unreachable!("ListDeployments yields ApplicationDeployments");
    };
    log_verbose(
        verbose,
        format!("list deployments of application {application_name}"),
    );
    println!(
        "{}",
        output::deployment_history(&application_name, &deployments)
    );
    Ok(())
}

// Selects the delivery mode requested by the deploy command options.
pub(crate) fn run_deploy(
    executor: &ControlExecutor,
    verbose: bool,
    application_name: &str,
    image_reference: Option<String>,
    branch: Option<String>,
) -> Result<(), CliError> {
    if let Some(branch) = branch {
        run_deploy_branch(executor, verbose, application_name, &branch)
    } else {
        let image_reference = image_reference.ok_or(CliError::MissingDeployOption)?;
        run_deploy_oci(executor, verbose, application_name, &image_reference)
    }
}

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
    let result = result.map_err(CliError::from_control)?;
    let CommandResult::ApplicationDeployed {
        application_name,
        deployment,
    } = result
    else {
        unreachable!("DeployImage yields ApplicationDeployed");
    };
    println!("{}", output::deployed(&application_name, &deployment));
    Ok(())
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
    let result = result.map_err(CliError::from_control)?;
    let CommandResult::ApplicationDeployed {
        application_name,
        deployment,
    } = result
    else {
        unreachable!("DeployBranch yields ApplicationDeployed");
    };
    println!("{}", output::deployed(&application_name, &deployment));
    Ok(())
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
    let result = result.map_err(CliError::from_control)?;
    let CommandResult::ApplicationRolledBack {
        application_name,
        deployment: rolled_back,
    } = result
    else {
        unreachable!("Rollback yields ApplicationRolledBack");
    };
    println!(
        "{}",
        output::rollback_result(&application_name, &rolled_back)
    );
    Ok(())
}
