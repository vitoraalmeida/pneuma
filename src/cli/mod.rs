mod args;
mod ci;
mod deployment;
mod error;
mod output;
mod progress;
mod shared;

use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor};

use error::CliError;
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
        InvocationTarget::MissingDeployOption => return Err(CliError::MissingDeployOption),
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

    match command {
        Command::DeployImage {
            application_name,
            image_reference,
        } => deployment::run_deploy_oci(&executor, verbose, &application_name, &image_reference),
        Command::DeployBranch {
            application_name,
            branch,
        } => deployment::run_deploy_branch(&executor, verbose, &application_name, &branch),
        Command::Rollback { application_name } => {
            deployment::run_rollback(&executor, verbose, &application_name)
        }
        command => execute_and_render(&executor, command, verbose),
    }
}

// Executes all non-deployment commands through one control-to-CLI adaptation path.
fn execute_and_render(
    executor: &ControlExecutor,
    command: Command,
    verbose: bool,
) -> Result<(), CliError> {
    log_command_start(&command, verbose);
    match executor.execute(command) {
        Ok(result) => render_command_result(result, verbose),
        Err(ControlError::DoctorConnection { source, report }) => {
            render_doctor_report(&report, verbose);
            Err(CliError::Database { source })
        }
        Err(source) => Err(CliError::from_control(source)),
    }
}

// Renders every boundary result without relying on the command that produced it.
pub(crate) fn render_command_result(result: CommandResult, verbose: bool) -> Result<(), CliError> {
    match result {
        CommandResult::SystemCreated(system) => println!("{}", output::created_system(&system)),
        CommandResult::Systems(systems) => print_nonempty(output::system_list(&systems)),
        CommandResult::SystemDetails(details) => println!("{}", output::system_details(&details)),
        CommandResult::ApplicationImported(application) => {
            println!("{}", output::imported_application(&application));
        }
        CommandResult::Applications(entries) => {
            print_nonempty(output::application_list(&entries).trim_end().to_owned());
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
        } => {
            println!("Application: {application_name}");
            println!("{}", output::runtime_status(&observation));
        }
        CommandResult::ApplicationStopped {
            application_name,
            observation,
        } => {
            println!("Stopped {application_name}");
            println!("{}", output::lifecycle_outcome(&observation));
        }
        CommandResult::ApplicationStarted {
            application_name,
            observation,
        } => {
            println!("Started {application_name}");
            println!("{}", output::lifecycle_outcome(&observation));
        }
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
                format!("set visibility of application {application_name} to {visibility:?}"),
            );
        }
        Command::Reconcile { application_name } => {
            log_verbose(
                verbose,
                format!("reconcile application: {application_name}"),
            );
        }
        Command::Doctor
        | Command::DatabaseBackup { .. }
        | Command::DatabaseRestore { .. }
        | Command::DeployImage { .. }
        | Command::DeployBranch { .. }
        | Command::Rollback { .. } => {}
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
pub(crate) fn run_version() {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use pneuma::adapters::diagnostics::DoctorReport;
    use pneuma::control::CommandResult;

    use super::{CliError, render_command_result};

    #[test]
    fn unhealthy_doctor_result_keeps_the_diagnostic_failure_class() {
        let report = DoctorReport::database_connection_failure(std::path::Path::new("/missing"));

        let error = render_command_result(CommandResult::Doctor(report), false)
            .expect_err("an unhealthy doctor report must fail the command");

        assert!(matches!(error, CliError::Doctor));
    }
}
