mod args;
mod ci;
mod error;
mod output;
mod progress;
mod shared;

use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor};

use error::CliError;
use progress::DeploymentProgressRenderer;
use shared::log_verbose;

pub(crate) use args::{Invocation, InvocationTarget, parse_invocation};

// Routes parsed commands into the interface-neutral control boundary.
pub(crate) fn run(invocation: Invocation) -> Result<(), CliError> {
    let Invocation { verbose, target } = invocation;

    let command = match target {
        InvocationTarget::Version => {
            run_version();
            return Ok(());
        }
        InvocationTarget::CiDispatch => {
            return ci::run_ci_dispatch(&ControlExecutor::from_environment(), verbose);
        }
        InvocationTarget::Control(command) => command,
    };

    let executor = ControlExecutor::from_environment();
    if !matches!(
        command,
        Command::Doctor | Command::DatabaseBackup { .. } | Command::DatabaseRestore { .. }
    ) {
        log_verbose(
            verbose,
            format!("database: {}", executor.host().database_path.display()),
        );
    }

    execute_control_command(&executor, command, verbose)
}

// Executes a control command and attaches CLI-only progress rendering when it deploys.
fn execute_control_command(
    executor: &ControlExecutor,
    command: Command,
    verbose: bool,
) -> Result<(), CliError> {
    log_command_start(&command, verbose);

    match deployment_request(&command) {
        Some(request) => execute_deployment_with_events(executor, command, request, verbose),
        None => execute_without_events(executor, command, verbose),
    }
}

// Classifies the deployment requests that use event-capable execution.
enum DeploymentRequest {
    Image(String),
    Branch(String),
    Rollback,
}

fn deployment_request(command: &Command) -> Option<DeploymentRequest> {
    match command {
        Command::DeployImage {
            image_reference, ..
        } => Some(DeploymentRequest::Image(image_reference.clone())),
        Command::DeployBranch { branch, .. } => Some(DeploymentRequest::Branch(branch.clone())),
        Command::Rollback { .. } => Some(DeploymentRequest::Rollback),
        _ => None,
    }
}

// Runs ordinary commands without constructing a terminal progress renderer.
fn execute_without_events(
    executor: &ControlExecutor,
    command: Command,
    verbose: bool,
) -> Result<(), CliError> {
    match executor.execute(command) {
        Ok(result) => render_command_result(result, verbose),
        Err(ControlError::DoctorConnection { source, report }) => {
            render_doctor_report(&report, verbose);
            Err(CliError::Database { source })
        }
        Err(source) => Err(CliError::from_control(source)),
    }
}

// Runs every deployment command through the same event-capable control invocation.
fn execute_deployment_with_events(
    executor: &ControlExecutor,
    command: Command,
    request: DeploymentRequest,
    verbose: bool,
) -> Result<(), CliError> {
    let requested_input = match &request {
        DeploymentRequest::Image(image_reference) => Some(("image", image_reference.as_str())),
        DeploymentRequest::Branch(branch) => Some(("branch", branch.as_str())),
        DeploymentRequest::Rollback => None,
    };
    let mut renderer = DeploymentProgressRenderer::new(verbose, requested_input);
    let mut events = |event| renderer.report(event);
    let result = executor.execute_with_events(command, &mut events);
    renderer.finish();
    render_command_result(result.map_err(CliError::from_control)?, verbose)
}

// Renders every boundary result without relying on the command that produced it.
fn render_command_result(result: CommandResult, verbose: bool) -> Result<(), CliError> {
    match result {
        CommandResult::SystemCreated(system) => println!("{}", output::created_system(&system)),
        CommandResult::Systems(systems) => print_nonempty(output::system_list(&systems)),
        CommandResult::SystemDetails(details) => println!("{}", output::system_details(&details)),
        CommandResult::ApplicationImported(application) => {
            println!("{}", output::imported_application(&application));
        }
        CommandResult::Applications(entries) => {
            print_nonempty(output::application_list(&entries));
        }
        CommandResult::ApplicationDeployments {
            application_name,
            deployments,
        } => {
            log_verbose(
                verbose,
                format!("list deployments of application {application_name}"),
            );
            println!(
                "{}",
                output::deployment_history(&application_name, &deployments)
            );
        }
        CommandResult::ApplicationStatus {
            application_name,
            observation,
        } => println!(
            "{}",
            output::application_status(&application_name, &observation)
        ),
        CommandResult::ApplicationStopped {
            application_name,
            observation,
        } => println!(
            "{}",
            output::application_stopped(&application_name, &observation)
        ),
        CommandResult::ApplicationStarted {
            application_name,
            observation,
        } => println!(
            "{}",
            output::application_started(&application_name, &observation)
        ),
        CommandResult::ApplicationDeployed {
            application_name,
            deployment,
        } => println!("{}", output::deployed(&application_name, &deployment)),
        CommandResult::ApplicationRolledBack {
            application_name,
            deployment,
        } => println!(
            "{}",
            output::rollback_result(&application_name, &deployment)
        ),
        CommandResult::ExposureChanged {
            application_name,
            change,
        } => println!("{}", output::visibility_change(&application_name, &change)),
        CommandResult::Reconciled {
            application_name,
            result,
        } => println!(
            "{}",
            output::reconciliation_result(&application_name, &result)
        ),
        CommandResult::Doctor(report) => {
            render_doctor_report(&report, verbose);
            if !report.is_healthy() {
                return Err(CliError::Doctor);
            }
        }
        CommandResult::DatabaseBackedUp { path } => println!("{}", output::database_backup(&path)),
        CommandResult::DatabaseRestored {
            path,
            pre_restore_path,
        } => println!("{}", output::database_restore(&path, &pre_restore_path)),
    }
    Ok(())
}

fn log_command_start(command: &Command, verbose: bool) {
    match command {
        Command::SystemCreate { name, .. } => {
            log_verbose(verbose, format!("create system: {name}"))
        }
        Command::SystemList => log_verbose(verbose, "list registered systems"),
        Command::SystemShow { name } => log_verbose(verbose, format!("show system: {name}")),
        Command::ImportApplication { repository, .. } => {
            log_verbose(verbose, format!("import repository: {repository}"));
        }
        Command::ListApplications => log_verbose(verbose, "list registered applications"),
        Command::ListDeployments { application_name } => {
            log_verbose(
                verbose,
                format!("resolve application by name: {application_name}"),
            );
        }
        Command::ApplicationStatus { application_name } => {
            log_verbose(
                verbose,
                format!("resolve application by name: {application_name}"),
            );
            log_verbose(
                verbose,
                format!("report status of application {application_name}"),
            );
        }
        Command::ApplicationStop { application_name } => {
            log_verbose(
                verbose,
                format!("resolve application by name: {application_name}"),
            );
            log_verbose(verbose, format!("stop application {application_name}"));
        }
        Command::ApplicationStart { application_name } => {
            log_verbose(
                verbose,
                format!("resolve application by name: {application_name}"),
            );
            log_verbose(verbose, format!("start application {application_name}"));
        }
        Command::VisibilitySet {
            application_name,
            visibility,
        } => {
            log_verbose(
                verbose,
                format!("resolve application by name: {application_name}"),
            );
            log_verbose(
                verbose,
                format!(
                    "set visibility of application {application_name} to {}",
                    output::visibility_label(*visibility)
                ),
            );
        }
        Command::Reconcile { application_name } => {
            log_verbose(
                verbose,
                format!("reconcile application: {application_name}"),
            );
        }
        Command::DeployImage {
            application_name, ..
        }
        | Command::DeployBranch {
            application_name, ..
        }
        | Command::Rollback { application_name } => {
            log_verbose(
                verbose,
                format!("resolve application by name: {application_name}"),
            );
        }
        Command::Doctor | Command::DatabaseBackup { .. } | Command::DatabaseRestore { .. } => {}
    }
}

fn print_nonempty(rendered: String) {
    if !rendered.is_empty() {
        println!("{rendered}");
    }
}

fn render_doctor_report(report: &pneuma::adapters::diagnostics::DoctorReport, verbose: bool) {
    for check in &report.checks {
        if let Some(label) = check.verbose_label() {
            log_verbose(verbose, label);
        }
    }
    println!("{}", output::doctor_report(report));
}

// Prints version information without requiring host configuration or database access.
fn run_version() {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use pneuma::adapters::diagnostics::DoctorReport;
    use pneuma::control::{Command, CommandResult};

    use super::{CliError, DeploymentRequest, deployment_request, render_command_result};

    #[test]
    fn deployment_classification_selects_event_capable_execution() {
        let image = Command::DeployImage {
            application_name: "portal".to_owned(),
            image_reference: "registry.example/portal@sha256:abc".to_owned(),
        };
        let branch = Command::DeployBranch {
            application_name: "portal".to_owned(),
            branch: "main".to_owned(),
        };
        let rollback = Command::Rollback {
            application_name: "portal".to_owned(),
        };
        let ordinary = Command::ApplicationStatus {
            application_name: "portal".to_owned(),
        };

        assert!(matches!(
            deployment_request(&image),
            Some(DeploymentRequest::Image(reference))
                if reference == "registry.example/portal@sha256:abc"
        ));
        assert!(matches!(
            deployment_request(&branch),
            Some(DeploymentRequest::Branch(branch)) if branch == "main"
        ));
        assert!(matches!(
            deployment_request(&rollback),
            Some(DeploymentRequest::Rollback)
        ));
        assert!(deployment_request(&ordinary).is_none());
    }

    #[test]
    fn unhealthy_doctor_result_keeps_the_diagnostic_failure_class() {
        let report = DoctorReport::database_connection_failure(std::path::Path::new("/missing"));

        let error = render_command_result(CommandResult::Doctor(report), false)
            .expect_err("an unhealthy doctor report must fail the command");

        assert!(matches!(error, CliError::Doctor));
    }
}
