mod application;
mod args;
mod ci;
mod deployment;
mod doctor;
mod error;
mod exposure;
mod output;
mod progress;
mod reconciliation;
mod shared;
mod system;

use pneuma::control::{Command, ControlExecutor};

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
        Command::SystemCreate { name, description } => {
            system::run_system_create(&executor, verbose, &name, description.as_deref())
        }
        Command::SystemList => system::run_system_list(&executor, verbose),
        Command::SystemShow { name } => system::run_system_show(&executor, verbose, &name),
        Command::ImportApplication {
            repository,
            system_name,
            manifest_path,
        } => application::run_import(
            &executor,
            verbose,
            &repository,
            system_name.as_deref(),
            manifest_path.as_deref(),
        ),
        Command::ListApplications => application::run_list(&executor, verbose),
        Command::ListDeployments { application_name } => {
            deployment::run_deployments(&executor, verbose, &application_name)
        }
        Command::ApplicationStatus { application_name } => {
            application::run_status(&executor, verbose, &application_name)
        }
        Command::ApplicationStop { application_name } => {
            application::run_stop(&executor, verbose, &application_name)
        }
        Command::ApplicationStart { application_name } => {
            application::run_start(&executor, verbose, &application_name)
        }
        Command::VisibilitySet {
            application_name,
            visibility,
        } => exposure::run_visibility_set(&executor, verbose, &application_name, visibility),
        Command::Reconcile { application_name } => {
            reconciliation::run_reconcile(&executor, verbose, &application_name)
        }
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
        Command::Doctor => doctor::run_doctor(&executor, verbose),
        Command::DatabaseBackup { path } => doctor::run_database_backup(&executor, &path),
        Command::DatabaseRestore { path } => doctor::run_database_restore(&executor, &path),
    }
}

// Prints version information without requiring host configuration or database access.
pub(crate) fn run_version() {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
}
