use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use pneuma::adapters::database::{self, DatabaseError};
use pneuma::adapters::diagnostics;
use pneuma::domain::application::Application;
use pneuma::domain::deployment::{DeploymentFailureEvidence, DeploymentLifecycle};
use pneuma::domain::exposure::Visibility;
use pneuma::domain::release::{InvalidOciArtifact, OciArtifact};
use pneuma::domain::system::{InvalidSystemName, SystemName};
use pneuma::use_cases::application::{
    ListError, LookupError, RemoteImportError, RuntimeLifecycleError, application_is_deployed,
    find_application_by_name, import_remote_application, list_applications,
    report_application_status, start_application, stop_application,
};
use pneuma::use_cases::ci::{CiCommand, CiDispatchError, parse_ci_command};
use pneuma::use_cases::deployment::{
    DeployBranchError, DeployOciError, DeploymentResult, ListDeploymentsError,
    PublicDeploymentConfiguration, RollbackError, deploy_branch, deploy_branch_with_progress,
    deploy_oci, deploy_oci_with_progress, list_deployments, rollback_deployment,
};
use pneuma::use_cases::exposure::{ExposureChangeError, change_exposure};
use pneuma::use_cases::reconciliation::{
    ReconciliationReadError, ReconciliationResult, reconcile_application,
};
use pneuma::use_cases::system::{create_system, list_systems, show_system};

const WORKSPACE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_WORKSPACE_PATH";
const DEFAULT_WORKSPACE_PATH: &str = "/var/lib/pneuma/checkouts";
const CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_CADDY_MANAGED_PATH";
const DEFAULT_CADDY_MANAGED_PATH: &str = "/etc/caddy/applications";
const CADDYFILE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_CADDYFILE_PATH";
const DEFAULT_CADDYFILE_PATH: &str = "/etc/caddy/Caddyfile";

#[derive(Parser)]
#[command(
    name = "pneuma",
    version,
    about = "Single-host container deployment CLI"
)]
// Captures Clap's public syntax before it is translated into internal commands.
struct Cli {
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage systems
    System {
        #[command(subcommand)]
        command: SystemCommands,
    },
    /// Manage applications
    App {
        #[command(subcommand)]
        command: AppCommands,
    },
    /// Manage deployments
    Deployment {
        #[command(subcommand)]
        command: DeploymentCommands,
    },
    /// Database operations
    Database {
        #[command(subcommand)]
        command: DatabaseCommands,
    },
    /// Print version information
    Version,
    /// Run diagnostic checks
    Doctor,
    /// Reconcile an application with its persisted intent
    Reconcile { application_name: String },
    /// CI dispatch (internal, via SSH)
    Ci {
        #[command(subcommand)]
        command: CiCommands,
    },
}

#[derive(Subcommand)]
enum SystemCommands {
    /// Create a new system
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// List all systems
    List,
    /// Show system details
    Show { name: String },
}

#[derive(Subcommand)]
enum AppCommands {
    /// Import an application
    Import {
        repository: String,
        #[arg(long)]
        system: Option<String>,
        #[arg(long)]
        manifest: Option<String>,
    },
    /// List all applications
    List,
    /// List deployments for an application
    Deployments { application_name: String },
    /// Show application status
    Status { application_name: String },
    /// Stop an application
    Stop { application_name: String },
    /// Start an application
    Start { application_name: String },
    /// Deploy an application
    Deploy {
        application_name: String,
        #[arg(long, conflicts_with = "branch")]
        image: Option<String>,
        #[arg(long, conflicts_with = "image")]
        branch: Option<String>,
    },
    /// Manage application visibility
    Visibility {
        #[command(subcommand)]
        command: VisibilityCommands,
    },
}

#[derive(Subcommand)]
enum VisibilityCommands {
    /// Set application visibility
    Set {
        application_name: String,
        visibility: VisibilityArg,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum VisibilityArg {
    Public,
    Internal,
}

impl From<VisibilityArg> for Visibility {
    fn from(arg: VisibilityArg) -> Self {
        match arg {
            VisibilityArg::Public => Visibility::Public,
            VisibilityArg::Internal => Visibility::Internal,
        }
    }
}

#[derive(Subcommand)]
enum DeploymentCommands {
    /// Rollback to previous deployment
    Rollback { application_name: String },
}

#[derive(Subcommand)]
enum DatabaseCommands {
    /// Backup database to file
    Backup { path: PathBuf },
    /// Restore database from file
    Restore { path: PathBuf },
}

#[derive(Subcommand)]
enum CiCommands {
    /// Dispatch CI command
    Dispatch,
}

impl From<Commands> for Command {
    fn from(cmd: Commands) -> Self {
        match cmd {
            Commands::System { command } => match command {
                SystemCommands::Create { name, description } => {
                    Command::SystemCreate { name, description }
                }
                SystemCommands::List => Command::SystemList,
                SystemCommands::Show { name } => Command::SystemShow { name },
            },
            Commands::App { command } => match command {
                AppCommands::Import {
                    repository,
                    system,
                    manifest,
                } => Command::Import {
                    repository,
                    system_name: system,
                    manifest_path: manifest,
                },
                AppCommands::List => Command::List,
                AppCommands::Deployments { application_name } => {
                    Command::Deployments { application_name }
                }
                AppCommands::Status { application_name } => Command::Status { application_name },
                AppCommands::Stop { application_name } => Command::Stop { application_name },
                AppCommands::Start { application_name } => Command::Start { application_name },
                AppCommands::Deploy {
                    application_name,
                    image,
                    branch,
                } => Command::Deploy {
                    application_name,
                    image_reference: image,
                    branch,
                },
                AppCommands::Visibility { command } => match command {
                    VisibilityCommands::Set {
                        application_name,
                        visibility,
                    } => Command::VisibilitySet {
                        application_name,
                        visibility: visibility.into(),
                    },
                },
            },
            Commands::Deployment { command } => match command {
                DeploymentCommands::Rollback { application_name } => {
                    Command::Rollback { application_name }
                }
            },
            Commands::Database { command } => match command {
                DatabaseCommands::Backup { path } => Command::DatabaseBackup { path },
                DatabaseCommands::Restore { path } => Command::DatabaseRestore { path },
            },
            Commands::Version => Command::Version,
            Commands::Doctor => Command::Doctor,
            Commands::Reconcile { application_name } => Command::Reconcile { application_name },
            Commands::Ci { .. } => Command::CiDispatch,
        }
    }
}

// Keeps global CLI settings alongside the normalized command for dispatch.
struct Invocation {
    verbose: bool,
    command: Command,
}

enum Command {
    SystemCreate {
        name: String,
        description: Option<String>,
    },
    SystemList,
    SystemShow {
        name: String,
    },
    Import {
        repository: String,
        system_name: Option<String>,
        manifest_path: Option<String>,
    },
    List,
    Deployments {
        application_name: String,
    },
    Status {
        application_name: String,
    },
    Stop {
        application_name: String,
    },
    Start {
        application_name: String,
    },
    Deploy {
        application_name: String,
        image_reference: Option<String>,
        branch: Option<String>,
    },
    Rollback {
        application_name: String,
    },
    VisibilitySet {
        application_name: String,
        visibility: Visibility,
    },
    Version,
    Doctor,
    DatabaseBackup {
        path: PathBuf,
    },
    DatabaseRestore {
        path: PathBuf,
    },
    Reconcile {
        application_name: String,
    },
    CiDispatch,
}

#[derive(Debug)]
enum CliError {
    Database {
        source: DatabaseError,
    },
    Import {
        source: RemoteImportError,
    },
    InvalidSystemName {
        source: InvalidSystemName,
    },
    List {
        source: ListError,
    },
    ApplicationLookup {
        source: LookupError,
    },
    ListDeployments {
        source: ListDeploymentsError,
    },
    ApplicationNotFound {
        application_name: String,
    },
    ApplicationRuntime {
        source: Box<RuntimeLifecycleError>,
    },
    DeployOci {
        source: Box<DeployOciError>,
    },
    InvalidOciArtifact {
        source: InvalidOciArtifact,
    },
    DeployBranch {
        source: Box<DeployBranchError>,
    },
    Rollback {
        source: RollbackError,
    },
    VisibilitySet {
        source: ExposureChangeError,
    },
    DatabaseBackup {
        source: DatabaseError,
    },
    DatabaseRestore {
        source: DatabaseError,
    },
    SystemCreate {
        source: pneuma::use_cases::system::CreateError,
    },
    SystemList {
        source: pneuma::use_cases::system::ListSystemsError,
    },
    SystemShow {
        source: pneuma::use_cases::system::ShowError,
    },
    CiDispatch {
        source: CiDispatchError,
    },
    Reconcile {
        source: ReconciliationReadError,
    },
    Doctor,
    MissingDeployOption,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database { source } => write!(formatter, "{source}"),
            Self::Import { source } => write!(formatter, "{source}"),
            Self::InvalidSystemName { source } => write!(formatter, "{source}"),
            Self::List { source } => write!(formatter, "{source}"),
            Self::ApplicationLookup { source } => write!(formatter, "{source}"),
            Self::ListDeployments { source } => write!(formatter, "{source}"),
            Self::ApplicationNotFound { application_name } => {
                write!(formatter, "application `{application_name}` was not found")
            }
            Self::ApplicationRuntime { source } => write!(formatter, "{source}"),
            Self::DeployOci { source } => write!(formatter, "{source}"),
            Self::InvalidOciArtifact { source } => write!(formatter, "{source}"),
            Self::DeployBranch { source } => write!(formatter, "{source}"),
            Self::Rollback { source } => write!(formatter, "{source}"),
            Self::VisibilitySet { source } => write!(formatter, "{source}"),
            Self::DatabaseBackup { source } | Self::DatabaseRestore { source } => {
                write!(formatter, "{source}")
            }
            Self::SystemCreate { source } => write!(formatter, "{source}"),
            Self::SystemList { source } => write!(formatter, "{source}"),
            Self::SystemShow { source } => write!(formatter, "{source}"),
            Self::CiDispatch { source } => write!(formatter, "{source}"),
            Self::Reconcile { source } => write!(formatter, "{source}"),
            Self::Doctor => formatter.write_str("one or more diagnostic checks failed"),
            Self::MissingDeployOption => {
                formatter.write_str("either --image or --branch must be specified")
            }
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database { source } => Some(source),
            Self::Import { source } => Some(source),
            Self::InvalidSystemName { source } => Some(source),
            Self::List { source } => Some(source),
            Self::ApplicationLookup { source } => Some(source),
            Self::ListDeployments { source } => Some(source),
            Self::DeployOci { source } => Some(source.as_ref()),
            Self::InvalidOciArtifact { source } => Some(source),
            Self::DeployBranch { source } => Some(source.as_ref()),
            Self::ApplicationRuntime { source } => Some(source.as_ref()),
            Self::ApplicationNotFound { .. } => None,
            Self::Rollback { source } => Some(source),
            Self::VisibilitySet { source } => Some(source),
            Self::DatabaseBackup { source } | Self::DatabaseRestore { source } => Some(source),
            Self::SystemCreate { source } => Some(source),
            Self::SystemList { source } => Some(source),
            Self::SystemShow { source } => Some(source),
            Self::CiDispatch { source } => Some(source),
            Self::Reconcile { source } => Some(source),
            Self::Doctor | Self::MissingDeployOption => None,
        }
    }
}

const HOST_ENVIRONMENT_FILE: &str = "/etc/pneuma/environment";

// Loads host defaults without overriding explicit environment supplied by the caller.
fn load_host_environment() {
    let content = match fs::read_to_string(HOST_ENVIRONMENT_FILE) {
        Ok(c) => c,
        Err(_) => return,
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if !key.is_empty() && env::var_os(key).is_none() {
                // SAFETY: called before any threads are spawned in main()
                unsafe { env::set_var(key, value) };
            }
        }
    }
}

// Initializes process-wide environment before parsing and dispatching the CLI request.
fn main() -> ExitCode {
    load_host_environment();
    configure_runtime_environment();

    let cli = Cli::parse();
    let invocation = Invocation {
        verbose: cli.verbose,
        command: cli.command.into(),
    };
    let result = run(invocation);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

// Routes commands so diagnostics, backup, restore, and SSH CI avoid unnecessary database work.
fn run(invocation: Invocation) -> Result<(), CliError> {
    let Invocation { verbose, command } = invocation;

    if matches!(
        command,
        Command::Version
            | Command::Doctor
            | Command::DatabaseBackup { .. }
            | Command::DatabaseRestore { .. }
    ) {
        let database_path = database::configured_path();

        if matches!(command, Command::Version) {
            return run_version();
        }
        if let Command::DatabaseBackup { path } = command {
            database::backup(&database_path, &path)
                .map_err(|source| CliError::DatabaseBackup { source })?;
            println!("Database backup: {}", path.display());
            return Ok(());
        }
        if let Command::DatabaseRestore { path } = command {
            let pre_restore = database::restore_and_verify(&database_path, &path)
                .map_err(|source| CliError::DatabaseRestore { source })?;
            println!("Database restored from {}", path.display());
            println!("Pre-restore backup: {}", pre_restore.display());
            return Ok(());
        }

        let connection = match database::open(&database_path) {
            Ok(conn) => conn,
            Err(source) => {
                println!(
                    "✗ Database connection: FAILED (unable to open database at {})",
                    database_path.display()
                );
                println!("\nSome checks failed. Please review the output above.");
                return Err(CliError::Database { source });
            }
        };
        return if diagnostics::run(&connection, verbose) {
            Ok(())
        } else {
            Err(CliError::Doctor)
        };
    }

    if matches!(command, Command::CiDispatch) {
        return run_ci_dispatch(verbose);
    }

    let database_path = database::configured_path();
    log_verbose(verbose, format!("database: {}", database_path.display()));
    let mut connection =
        database::open(&database_path).map_err(|source| CliError::Database { source })?;

    match command {
        Command::SystemCreate { name, description } => {
            run_system_create(&mut connection, verbose, &name, description.as_deref())
        }
        Command::SystemList => run_system_list(&connection, verbose),
        Command::SystemShow { name } => run_system_show(&connection, verbose, &name),
        Command::Import {
            repository,
            system_name,
            manifest_path,
        } => run_import(
            &mut connection,
            verbose,
            &repository,
            system_name.as_deref(),
            manifest_path.as_deref(),
        ),
        Command::List => run_list(&connection, verbose),
        Command::Deployments { application_name } => {
            run_deployments(&connection, verbose, &application_name)
        }
        Command::Status { application_name } => {
            run_status(&mut connection, verbose, &application_name)
        }
        Command::Stop { application_name } => run_stop(&mut connection, verbose, &application_name),
        Command::Start { application_name } => {
            run_start(&mut connection, verbose, &application_name)
        }
        Command::Deploy {
            application_name,
            image_reference,
            branch,
        } => {
            if let Some(branch) = branch {
                run_deploy_branch(&mut connection, verbose, &application_name, &branch)
            } else {
                let image_reference = image_reference.ok_or(CliError::MissingDeployOption)?;
                run_deploy_oci(
                    &mut connection,
                    verbose,
                    &application_name,
                    &image_reference,
                )
            }
        }
        Command::Rollback { application_name } => {
            run_rollback(&mut connection, verbose, &application_name)
        }
        Command::VisibilitySet {
            application_name,
            visibility,
        } => run_visibility_set(&mut connection, verbose, &application_name, visibility),
        Command::Reconcile { application_name } => {
            run_reconcile(&mut connection, verbose, &application_name)
        }
        Command::Doctor
        | Command::Version
        | Command::DatabaseBackup { .. }
        | Command::DatabaseRestore { .. }
        | Command::CiDispatch => unreachable!(),
    }
}

// Emits operational detail only when the global verbose flag is enabled.
fn log_verbose(verbose: bool, message: impl std::fmt::Display) {
    if verbose {
        eprintln!("[verbose] {message}");
    }
}

// Clones only remote repositories into an isolated checkout, then always attempts cleanup.
fn run_import(
    connection: &mut rusqlite::Connection,
    verbose: bool,
    repository: &str,
    system_name: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("import repository: {repository}"));
    let workspace = configured_path(WORKSPACE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_WORKSPACE_PATH);
    let application = import_remote_application(
        connection,
        repository,
        &workspace,
        system_name,
        manifest_path,
    )
    .map_err(|source| CliError::Import { source });
    let application = application?;
    println!("Imported {}", application.name);
    println!("Status: Registered");
    if let Some(deployment_id) = &application.active_deployment_id {
        println!("Deployment: {deployment_id}");
    } else {
        println!("Deployment: Not deployed");
    }
    Ok(())
}

// Adapts system creation results and errors to the CLI's output contract.
fn run_system_create(
    connection: &mut rusqlite::Connection,
    verbose: bool,
    name: &str,
    description: Option<&str>,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("create system: {name}"));
    let name = SystemName::new(name).map_err(|source| CliError::InvalidSystemName { source })?;
    let system = create_system(connection, &name, description)
        .map_err(|source| CliError::SystemCreate { source })?;
    println!("Created {}", system.name);
    Ok(())
}

// Renders registered systems without adding CLI-layer filtering.
fn run_system_list(connection: &rusqlite::Connection, verbose: bool) -> Result<(), CliError> {
    log_verbose(verbose, "list registered systems");
    let systems = list_systems(connection).map_err(|source| CliError::SystemList { source })?;
    for system in systems {
        println!("{}", system.name);
    }
    Ok(())
}

// Renders the system detail view returned by the use case.
fn run_system_show(
    connection: &rusqlite::Connection,
    verbose: bool,
    name: &str,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("show system: {name}"));
    let name = SystemName::new(name).map_err(|source| CliError::InvalidSystemName { source })?;
    let details =
        show_system(connection, &name).map_err(|source| CliError::SystemShow { source })?;
    println!("System: {}", details.system.name);
    if let Some(description) = &details.system.description {
        println!("Description: {description}");
    }
    if details.applications.is_empty() {
        println!("Applications: (none)");
    } else {
        println!("Applications:");
        for application in &details.applications {
            println!("  {}", application.name);
        }
    }
    Ok(())
}

// Reports each registered application's persisted deployment state.
fn run_list(connection: &rusqlite::Connection, verbose: bool) -> Result<(), CliError> {
    log_verbose(verbose, "list registered applications");
    let applications = list_applications(connection).map_err(|source| CliError::List { source })?;
    for application in applications {
        let deployment_status = if application_is_deployed(connection, &application.id)
            .map_err(|source| CliError::List { source })?
        {
            "Deployed"
        } else {
            "Not deployed"
        };

        println!("{}\tRegistered\t{deployment_status}", application.name);
    }
    Ok(())
}

// Resolves the named application before listing only its deployment history.
fn run_deployments(
    connection: &rusqlite::Connection,
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
        format!("list deployments of application {}", application.name),
    );
    let deployments = list_deployments(connection, &application.id)
        .map_err(|source| CliError::ListDeployments { source })?;
    if deployments.is_empty() {
        println!("No deployments for {}", application.name);
    } else {
        println!("Deployments for {}:", application.name);
        println!("DEPLOYMENT\tTYPE\tRELEASE\tSOURCE\tSTATUS\tSTARTED\tFINISHED\tACTIVE\tFAILURE");
        for deployment in deployments {
            let source = deployment
                .deployment
                .source_revision
                .as_ref()
                .map_or("-", pneuma::domain::deployment::SourceRevision::as_str);
            let (finished_at, failure) = match &deployment.deployment.lifecycle {
                DeploymentLifecycle::Succeeded { finished_at } => {
                    (finished_at.as_str(), "-".to_owned())
                }
                DeploymentLifecycle::Failed {
                    evidence: DeploymentFailureEvidence::Complete(failure),
                } => (
                    failure.finished_at.as_str(),
                    format!("{}:{}:{}", failure.code, failure.stage, failure.message),
                ),
                DeploymentLifecycle::Failed {
                    evidence: DeploymentFailureEvidence::Incomplete,
                } => ("-", "incomplete".to_owned()),
                DeploymentLifecycle::Pending
                | DeploymentLifecycle::Starting
                | DeploymentLifecycle::Verifying
                | DeploymentLifecycle::Activating => ("-", "-".to_owned()),
            };
            println!(
                "{}\t{:?}\t{}\t{}\t{:?}\t{}\t{}\t{}\t{}",
                deployment.deployment.id,
                deployment.deployment.deployment_type,
                deployment.release.artifact.digest(),
                source,
                deployment.deployment.status(),
                deployment.deployment.started_at.as_deref().unwrap_or("-"),
                finished_at,
                if deployment.is_active { "yes" } else { "no" },
                failure,
            );
        }
    }
    Ok(())
}

// Queries runtime status through the use case, which may inspect external runtime state.
fn run_status(
    connection: &mut rusqlite::Connection,
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
        format!("report status of application {}", application.name),
    );
    let observation = report_application_status(connection, &application.id, &application.name)
        .map_err(|source| CliError::ApplicationRuntime {
            source: Box::new(source),
        })?;
    println!("Application: {}", application.name);
    println!("Desired state: {:?}", observation.desired_runtime_state);
    println!("Observed state: {:?}", observation.observed_runtime_state);
    println!("Runtime: {}", observation.runtime_id);
    println!("Container: {}", observation.container_id);
    Ok(())
}

// Reconciles persisted runtime and exposure intent through configured host integrations.
fn run_reconcile(
    connection: &mut rusqlite::Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("reconcile application: {application_name}"),
    );
    let managed_caddy_directory = configured_path(
        CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
        DEFAULT_CADDY_MANAGED_PATH,
    );
    let caddyfile_path =
        configured_path(CADDYFILE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_CADDYFILE_PATH);
    let application_name = pneuma::domain::application::ApplicationName::new(application_name)
        .map_err(|_| CliError::Reconcile {
            source:
                pneuma::use_cases::reconciliation::ReconciliationReadError::ApplicationNotFound {
                    application_name: application_name.to_owned(),
                },
        })?;
    match reconcile_application(
        connection,
        &application_name,
        &managed_caddy_directory,
        &caddyfile_path,
    )
    .map_err(|source| CliError::Reconcile { source })?
    {
        ReconciliationResult::NoOp => {
            println!("Application: {application_name}");
            println!("Result: no-op");
        }
        ReconciliationResult::Deferred {
            blocking_deployment,
        } => {
            println!("Application: {application_name}");
            println!("Result: deferred");
            if let Some(blocking_deployment) = blocking_deployment {
                println!(
                    "Blocking deployment: {} ({})",
                    blocking_deployment.id,
                    blocking_deployment.status()
                );
            }
        }
        ReconciliationResult::Repaired {
            runtime_id,
            container_id,
        } => {
            println!("Application: {application_name}");
            println!("Result: repaired");
            println!("Runtime: {runtime_id}");
            println!("Container: {container_id}");
        }
        ReconciliationResult::ManualIntervention { reason } => {
            println!("Application: {application_name}");
            println!("Result: manual-intervention");
            println!("Diagnostic: {reason}");
        }
        ReconciliationResult::ExposureRepaired => {
            println!("Application: {application_name}");
            println!("Result: repaired");
        }
        ReconciliationResult::Failed { reason } => {
            println!("Application: {application_name}");
            println!("Result: failed");
            println!("Diagnostic: {reason}");
        }
        ReconciliationResult::Diverged { reason } => {
            println!("Application: {application_name}");
            println!("Result: diverged");
            println!("Diagnostic: {reason}");
        }
    }
    Ok(())
}

// Requests a runtime stop through the use case and reports its resulting observation.
fn run_stop(
    connection: &mut rusqlite::Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    log_verbose(verbose, format!("stop application {}", application.name));
    let observation =
        stop_application(connection, &application.id, &application.name).map_err(|source| {
            CliError::ApplicationRuntime {
                source: Box::new(source),
            }
        })?;
    println!("Stopped {}", application.name);
    println!("Desired state: {:?}", observation.desired_runtime_state);
    println!("Observed state: {:?}", observation.observed_runtime_state);
    Ok(())
}

// Requests a runtime start through the use case and reports its resulting observation.
fn run_start(
    connection: &mut rusqlite::Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    log_verbose(verbose, format!("start application {}", application.name));
    let observation =
        start_application(connection, &application.id, &application.name).map_err(|source| {
            CliError::ApplicationRuntime {
                source: Box::new(source),
            }
        })?;
    println!("Started {}", application.name);
    println!("Desired state: {:?}", observation.desired_runtime_state);
    println!("Observed state: {:?}", observation.observed_runtime_state);
    Ok(())
}

// Deploys a supplied OCI reference with host-configured public exposure paths.
fn run_deploy_oci(
    connection: &mut rusqlite::Connection,
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
    let deployed = if verbose {
        let mut progress = |event| eprintln!("{event}");
        deploy_oci_with_progress(
            connection,
            &application.id,
            &artifact,
            None,
            Some(&public_configuration),
            &mut progress,
        )
    } else {
        deploy_oci(
            connection,
            &application.id,
            &artifact,
            None,
            Some(&public_configuration),
        )
    }
    .map_err(|source| CliError::DeployOci {
        source: Box::new(source),
    })?;
    print_deployed(&application.name, &deployed);
    Ok(())
}

// Resolves and deploys the requested branch's published OCI artifact with host-configured paths.
fn run_deploy_branch(
    connection: &mut rusqlite::Connection,
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
    let deployed = if verbose {
        let mut progress = |event| eprintln!("{event}");
        deploy_branch_with_progress(
            connection,
            &application.id,
            Some(branch),
            Some(&public_configuration),
            &mut progress,
        )
    } else {
        deploy_branch(
            connection,
            &application.id,
            Some(branch),
            Some(&public_configuration),
        )
    }
    .map_err(|source| CliError::DeployBranch {
        source: Box::new(source),
    })?;
    print_deployed(&application.name, &deployed);
    Ok(())
}

// Rolls back through the use case while supplying paths needed for public exposure effects.
fn run_rollback(
    connection: &mut rusqlite::Connection,
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
    let public_configuration = PublicDeploymentConfiguration {
        managed_caddy_directory: configured_path(
            CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
            DEFAULT_CADDY_MANAGED_PATH,
        ),
        caddyfile_path: configured_path(
            CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
            DEFAULT_CADDYFILE_PATH,
        ),
    };
    let rolled_back = rollback_deployment(connection, &application.id, Some(&public_configuration))
        .map_err(|source| CliError::Rollback { source })?;
    println!("Rolled back {}", application.name);
    println!("Image: {}", rolled_back.artifact.reference());
    if let Some(source_revision) = rolled_back.source_revision {
        println!("Source revision: {source_revision}");
    }
    println!("Deployment: {}", rolled_back.deployment_id);
    println!("Runtime: {}", rolled_back.runtime_id);
    println!("Status: Succeeded");
    Ok(())
}

// Changes visibility through the use case, which manages the Caddy side effects.
fn run_visibility_set(
    connection: &mut rusqlite::Connection,
    verbose: bool,
    application_name: &str,
    visibility: Visibility,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    let managed_directory = configured_path(
        CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
        DEFAULT_CADDY_MANAGED_PATH,
    );
    let caddyfile_path =
        configured_path(CADDYFILE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_CADDYFILE_PATH);
    log_verbose(
        verbose,
        format!(
            "set visibility of application {} to {:?}",
            application.name, visibility
        ),
    );
    let exposure_change = change_exposure(
        connection,
        &application.id,
        visibility,
        &managed_directory,
        &caddyfile_path,
    )
    .map_err(|source| CliError::VisibilitySet { source })?;
    match exposure_change.visibility {
        Visibility::Public => {
            println!("Visibility for {}: Public", application.name);
            if let Some(domain) = exposure_change.domain {
                println!("Domain: {}", domain);
            }
        }
        Visibility::Internal => {
            println!("Visibility for {}: Internal", application.name);
        }
    }
    Ok(())
}

// Resolves optional path overrides consistently, treating an empty value as unset.
fn configured_path(variable: &str, default: &str) -> PathBuf {
    env::var_os(variable)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
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

fn print_deployed(
    application_name: &pneuma::domain::application::ApplicationName,
    deployed: &DeploymentResult,
) {
    println!("Deployed {application_name}");
    println!("Image: {}", deployed.artifact.reference());
    if let Some(source_revision) = &deployed.source_revision {
        println!("Source revision: {source_revision}");
    }
    println!("Deployment: {}", deployed.deployment_id);
    println!("Runtime: {}", deployed.runtime_id);
    println!("Container: {}", deployed.container_name);
    println!("Status: Succeeded");
}

// Converts expected absence from the store-facing use case into a CLI-specific error.
fn resolve_application(
    connection: &rusqlite::Connection,
    application_name: &str,
) -> Result<Application, CliError> {
    let application_name = pneuma::domain::application::ApplicationName::new(application_name)
        .map_err(|_| CliError::ApplicationNotFound {
            application_name: application_name.to_owned(),
        })?;
    find_application_by_name(connection, &application_name)
        .map_err(|source| CliError::ApplicationLookup { source })?
        .ok_or_else(|| CliError::ApplicationNotFound {
            application_name: application_name.as_str().to_owned(),
        })
}

// Prints version information without requiring host configuration or database access.
fn run_version() -> Result<(), CliError> {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

// Derives uid-scoped runtime paths so rootless systemd and Podman never use another user's bus.
fn configure_runtime_environment() {
    let uid = unsafe { libc::getuid() };
    let runtime_dir = format!("/run/user/{}", uid);
    let dbus_address = format!("unix:path={}/bus", runtime_dir);

    // XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS are uid-scoped: a value inherited
    // from another user (for example /run/user/0 when launched through `runuser` as
    // root) is never valid for this process, so they are always derived from the
    // effective uid.
    unsafe {
        env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
        env::set_var("DBUS_SESSION_BUS_ADDRESS", &dbus_address);
    }

    if let Ok(home) = env::var("HOME") {
        let quadlet_dir = format!("{}/.config/containers/systemd", home);
        if env::var_os("PNEUMA_QUADLET_DIR").is_none() {
            unsafe {
                env::set_var("PNEUMA_QUADLET_DIR", &quadlet_dir);
            }
        }
    }
}

// Restricts SSH CI execution to the validated command carried by SSH_ORIGINAL_COMMAND.
fn run_ci_dispatch(verbose: bool) -> Result<(), CliError> {
    let original_command = env::var("SSH_ORIGINAL_COMMAND").map_err(|_| CliError::CiDispatch {
        source: CiDispatchError::MissingSshOriginalCommand,
    })?;

    log_verbose(verbose, format!("CI command: {original_command}"));

    let ci_command =
        parse_ci_command(&original_command).map_err(|source| CliError::CiDispatch { source })?;

    match ci_command {
        CiCommand::Version => run_version(),
        CiCommand::Deploy {
            application,
            branch,
        } => {
            let database_path = database::configured_path();

            let mut connection =
                database::open(&database_path).map_err(|source| CliError::Database { source })?;

            run_deploy_branch(&mut connection, verbose, &application, &branch)
        }
    }
}
