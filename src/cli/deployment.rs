use rusqlite::Connection;

use pneuma::control::{Command, CommandResult, ControlExecutor};
use pneuma::domain::release::OciArtifact;
use pneuma::use_cases::deployment::{
    DeployBranchError, DeployOciError, DeploymentEvent, DeploymentStep,
    PublicDeploymentConfiguration, RetirementWarning, deploy_branch_with_events,
    deploy_oci_with_events, rollback_deployment_with_events,
};

use super::error::CliError;
use super::output;
use super::shared::{
    CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE, CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
    DEFAULT_CADDY_MANAGED_PATH, DEFAULT_CADDYFILE_PATH, configured_path, log_verbose,
    resolve_application,
};

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
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
    image_reference: Option<String>,
    branch: Option<String>,
) -> Result<(), CliError> {
    if let Some(branch) = branch {
        run_deploy_branch(connection, verbose, application_name, &branch)
    } else {
        let image_reference = image_reference.ok_or(CliError::MissingDeployOption)?;
        run_deploy_oci(connection, verbose, application_name, &image_reference)
    }
}

// Deploys a supplied OCI reference with host-configured public exposure paths.
pub(crate) fn run_deploy_oci(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
    image_reference: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let artifact = OciArtifact::parse(image_reference)
        .map_err(|source| CliError::InvalidOciArtifact { source })?;
    let application = resolve_application(connection, application_name)?;
    let public_configuration = public_deployment_configuration();
    if verbose {
        log_verbose(
            verbose,
            format!(
                "deployment input: application {}, image {image_reference}",
                application.name
            ),
        );
    } else {
        eprintln!("Deploying {}...", application.name);
    }
    let mut events = |event| render_deployment_event_if_visible(&event, verbose);
    let deployed = deploy_oci_with_events(
        connection,
        &application.id,
        &artifact,
        None,
        Some(&public_configuration),
        &mut events,
    )
    .map_err(|source: DeployOciError| CliError::DeployOci {
        source: Box::new(source),
    })?;
    println!("{}", output::deployed(&application.name, &deployed));
    Ok(())
}

fn public_deployment_configuration() -> PublicDeploymentConfiguration {
    PublicDeploymentConfiguration {
        managed_caddy_directory: configured_path(
            CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
            DEFAULT_CADDY_MANAGED_PATH,
        ),
        caddyfile_path: configured_path(
            CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
            DEFAULT_CADDYFILE_PATH,
        ),
    }
}

// Resolves and deploys the requested branch's published OCI artifact with host-configured paths.
pub(crate) fn run_deploy_branch(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
    branch: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    let public_configuration = public_deployment_configuration();
    if verbose {
        log_verbose(
            verbose,
            format!(
                "deployment input: application {}, branch {branch}",
                application.name
            ),
        );
    } else {
        eprintln!("Deploying {}...", application.name);
    }
    let mut events = |event| render_deployment_event_if_visible(&event, verbose);
    let deployed = deploy_branch_with_events(
        connection,
        &application.id,
        Some(branch),
        Some(&public_configuration),
        &mut events,
    )
    .map_err(|source: DeployBranchError| CliError::DeployBranch {
        source: Box::new(source),
    })?;
    println!("{}", output::deployed(&application.name, &deployed));
    Ok(())
}

// Rolls back through the use case while supplying paths needed for public exposure effects.
pub(crate) fn run_rollback(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    log_verbose(
        verbose,
        format!("rolling back application {}", application.name),
    );
    let public_configuration = public_deployment_configuration();
    let mut events = |event| render_deployment_event_if_visible(&event, verbose);
    let rolled_back = rollback_deployment_with_events(
        connection,
        &application.id,
        Some(&public_configuration),
        &mut events,
    )
    .map_err(|source| CliError::Rollback { source })?;
    println!(
        "{}",
        output::rollback_result(&application.name, &rolled_back)
    );
    Ok(())
}

fn render_deployment_event_if_visible(event: &DeploymentEvent, verbose: bool) {
    if verbose || matches!(event, DeploymentEvent::RetirementWarning { .. }) {
        eprintln!("{}", render_deployment_event(event));
    }
}

// Renders use-case events with the CLI's stable text vocabulary.
fn render_deployment_event(event: &DeploymentEvent) -> String {
    match event {
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
