use pneuma::control::{Command, CommandResult, ControlExecutor};
use pneuma::use_cases::deployment::{DeploymentEvent, DeploymentStep, RetirementWarning};

use super::error::CliError;
use super::output;
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
    let mut events = |event| {
        render_deployment_event_if_visible(&event, verbose, Some(("image", image_reference)))
    };
    let result = executor
        .execute_with_events(
            Command::DeployImage {
                application_name: application_name.to_owned(),
                image_reference: image_reference.to_owned(),
            },
            &mut events,
        )
        .map_err(CliError::from_control)?;
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
    let mut events =
        |event| render_deployment_event_if_visible(&event, verbose, Some(("branch", branch)));
    let result = executor
        .execute_with_events(
            Command::DeployBranch {
                application_name: application_name.to_owned(),
                branch: branch.to_owned(),
            },
            &mut events,
        )
        .map_err(CliError::from_control)?;
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
    let mut events = |event| render_deployment_event_if_visible(&event, verbose, None);
    let result = executor
        .execute_with_events(
            Command::Rollback {
                application_name: application_name.to_owned(),
            },
            &mut events,
        )
        .map_err(CliError::from_control)?;
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

fn render_deployment_event_if_visible(
    event: &DeploymentEvent,
    verbose: bool,
    requested_input: Option<(&str, &str)>,
) {
    if let DeploymentEvent::DeploymentRequested { application_name } = event {
        match requested_input {
            Some((input_kind, input)) => {
                if verbose {
                    log_verbose(
                        true,
                        format!(
                            "deployment input: application {application_name}, {input_kind} {input}"
                        ),
                    );
                } else {
                    eprintln!("Deploying {application_name}...");
                }
            }
            None => log_verbose(
                verbose,
                format!("rolling back application {application_name}"),
            ),
        }
        return;
    }
    if verbose || matches!(event, DeploymentEvent::RetirementWarning { .. }) {
        eprintln!("{}", render_deployment_event(event));
    }
}

// Renders use-case events with the CLI's stable text vocabulary.
fn render_deployment_event(event: &DeploymentEvent) -> String {
    match event {
        DeploymentEvent::DeploymentRequested { .. } => {
            unreachable!("deployment requests are rendered separately")
        }
        DeploymentEvent::StepStarted { step } => {
            format!("{}: started", deployment_step_label(*step))
        }
        DeploymentEvent::StepCompleted { step } => {
            format!("{}: completed", deployment_step_label(*step))
        }
        DeploymentEvent::StateChanged {
            deployment_id,
            status,
        } => format!("deployment {deployment_id}: state changed to {status:?}"),
        DeploymentEvent::FailurePersisted {
            deployment_id,
            code,
        } => format!(
            "deployment {deployment_id}: state changed to Failed; failure persisted ({code})"
        ),
        DeploymentEvent::RetirementWarning {
            runtime_id,
            warning,
        } => match warning {
            RetirementWarning::UnitRetirementFailed { diagnostic } => {
                format!("warning: previous runtime {runtime_id} could not be retired: {diagnostic}")
            }
            RetirementWarning::ContainerRemovalUnproven { diagnostic } => format!(
                "warning: previous runtime {runtime_id} unit was retired but its container removal could not be proven: {diagnostic}"
            ),
            RetirementWarning::PersistenceFailed => format!(
                "warning: previous runtime {runtime_id} was retired but could not be marked removed"
            ),
        },
    }
}

fn deployment_step_label(step: DeploymentStep) -> &'static str {
    match step {
        DeploymentStep::ResolveBranch => "resolve branch",
        DeploymentStep::ResolveImageDigest => "resolve image digest",
        DeploymentStep::PullImage => "pull image",
        DeploymentStep::LoadSpecification => "load application specification",
        DeploymentStep::CreateDeployment => "create deployment",
        DeploymentStep::ReservePort => "reserve runtime port",
        DeploymentStep::CreateUnit => "create candidate unit",
        DeploymentStep::ReloadSystemd => "reload systemd user manager",
        DeploymentStep::StartContainer => "start candidate container",
        DeploymentStep::ResolveContainer => "resolve candidate container",
        DeploymentStep::ObserveContainer => "observe candidate container",
        DeploymentStep::RegisterCandidate => "register candidate runtime",
        DeploymentStep::InternalHealthCheck => "internal health check",
        DeploymentStep::ApplyPublicRoute => "apply public route",
        DeploymentStep::ExternalHealthCheck => "external health check",
        DeploymentStep::PromoteCandidate => "health check and promotion",
        DeploymentStep::CleanupCandidate => "clean up candidate",
        DeploymentStep::RetirePreviousRuntime => "retire previous runtime",
    }
}
