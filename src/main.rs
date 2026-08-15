use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand, ValueEnum};
use pneuma::adapters::database::{self, DatabaseError};
use pneuma::adapters::git_source::{
    CloneRepositoryError, cleanup_checkout, clone_repository, is_remote_repository,
};
use pneuma::adapters::oci_image::{OciImageReference, pull_image};
use pneuma::domain::application::Application;
use pneuma::domain::manifest::Visibility;
use pneuma::use_cases::application_import::{ImportError, import_application};
use pneuma::use_cases::application_list::{
    ListError, application_is_deployed, find_application_by_name, list_applications,
};
use pneuma::use_cases::application_runtime::{
    RuntimeLifecycleError, report_application_status, start_application, stop_application,
};
use pneuma::use_cases::ci_command::{CiCommand, CiDispatchError, parse_ci_command};
use pneuma::use_cases::deployment_execute_release::PublicDeploymentConfiguration;
use pneuma::use_cases::deployment_from_oci::{DeployOciError, deploy_oci};
use pneuma::use_cases::deployment_from_revision::{DeployBranchError, deploy_branch};
use pneuma::use_cases::deployment_list::{ListDeploymentsError, list_deployments};
use pneuma::use_cases::deployment_rollback::{RollbackError, rollback_deployment};
use pneuma::use_cases::exposure_change::{ExposureChangeError, change_exposure};
use pneuma::use_cases::system_create::create_system;
use pneuma::use_cases::system_list::list_systems;
use pneuma::use_cases::system_show::show_system;

const DATABASE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_DATABASE_PATH";
const DEFAULT_DATABASE_PATH: &str = "/var/lib/pneuma/database/pneuma.sqlite3";
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
            Commands::Ci { .. } => Command::CiDispatch,
        }
    }
}

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
    CiDispatch,
}

#[derive(Debug)]
enum CliError {
    Database {
        source: DatabaseError,
    },
    Import {
        source: ImportError,
    },
    ImportSource {
        source: CloneRepositoryError,
    },
    ImportWorkspace {
        source: std::io::Error,
    },
    InvalidImportRepository,
    List {
        source: ListError,
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
        source: pneuma::use_cases::system_create::CreateError,
    },
    SystemList {
        source: pneuma::use_cases::system_list::ListSystemsError,
    },
    SystemShow {
        source: pneuma::use_cases::system_show::ShowError,
    },
    CiDispatch {
        source: CiDispatchError,
    },
    Doctor,
    MissingDeployOption,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database { source } => write!(formatter, "{source}"),
            Self::Import { source } => write!(formatter, "{source}"),
            Self::ImportSource { source } => write!(formatter, "{source}"),
            Self::ImportWorkspace { source } => {
                write!(
                    formatter,
                    "failed to prepare the import workspace: {source}"
                )
            }
            Self::InvalidImportRepository => {
                write!(
                    formatter,
                    "application imports require a Git URL; local paths are not supported"
                )
            }
            Self::List { source } => write!(formatter, "{source}"),
            Self::ListDeployments { source } => write!(formatter, "{source}"),
            Self::ApplicationNotFound { application_name } => {
                write!(formatter, "application `{application_name}` was not found")
            }
            Self::ApplicationRuntime { source } => write!(formatter, "{source}"),
            Self::DeployOci { source } => write!(formatter, "{source}"),
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
            Self::ImportSource { source } => Some(source),
            Self::ImportWorkspace { source } => Some(source),
            Self::List { source } => Some(source),
            Self::ListDeployments { source } => Some(source),
            Self::DeployOci { source } => Some(source.as_ref()),
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
            Self::Doctor | Self::MissingDeployOption | Self::InvalidImportRepository => None,
        }
    }
}

const HOST_ENVIRONMENT_FILE: &str = "/etc/pneuma/environment";

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

fn run(invocation: Invocation) -> Result<(), CliError> {
    let Invocation { verbose, command } = invocation;

    if matches!(
        command,
        Command::Version
            | Command::Doctor
            | Command::DatabaseBackup { .. }
            | Command::DatabaseRestore { .. }
    ) {
        let database_path = env::var_os(DATABASE_PATH_ENVIRONMENT_VARIABLE)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATABASE_PATH));

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
            let pre_restore = database::restore(&database_path, &path)
                .map_err(|source| CliError::DatabaseRestore { source })?;
            let _ = database::open(&database_path)
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
        return run_doctor(&connection, verbose);
    }

    if matches!(command, Command::CiDispatch) {
        return run_ci_dispatch(verbose);
    }

    let database_path = env::var_os(DATABASE_PATH_ENVIRONMENT_VARIABLE)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATABASE_PATH));
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
        Command::Doctor
        | Command::Version
        | Command::DatabaseBackup { .. }
        | Command::DatabaseRestore { .. }
        | Command::CiDispatch => unreachable!(),
    }
}

fn log_verbose(verbose: bool, message: impl std::fmt::Display) {
    if verbose {
        eprintln!("[verbose] {message}");
    }
}

fn run_import(
    connection: &mut rusqlite::Connection,
    verbose: bool,
    repository: &str,
    system_name: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<(), CliError> {
    if !is_remote_repository(repository) {
        return Err(CliError::InvalidImportRepository);
    }

    log_verbose(verbose, format!("import repository: {repository}"));
    let workspace = configured_path(WORKSPACE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_WORKSPACE_PATH);
    let temporary_root = workspace.join("imports");
    fs::create_dir_all(&temporary_root).map_err(|source| CliError::ImportWorkspace { source })?;
    let checkout = temporary_root.join(unique_suffix());
    if let Err(source) = clone_repository(repository, &checkout) {
        let _ = cleanup_checkout(&checkout);
        return Err(CliError::ImportSource { source });
    }

    let import_result = import_application(
        connection,
        &checkout,
        system_name,
        Some(repository),
        manifest_path,
    )
    .map_err(|source| CliError::Import { source });

    let _ = cleanup_checkout(&checkout);

    let application = import_result?;
    println!("Imported {}", application.name);
    println!("Status: Registered");
    if let Some(deployment_id) = &application.active_deployment_id {
        println!("Deployment: {deployment_id}");
    } else {
        println!("Deployment: Not deployed");
    }
    Ok(())
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

fn run_system_create(
    connection: &mut rusqlite::Connection,
    verbose: bool,
    name: &str,
    description: Option<&str>,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("create system: {name}"));
    let system = create_system(connection, name, description)
        .map_err(|source| CliError::SystemCreate { source })?;
    println!("Created {}", system.name);
    Ok(())
}

fn run_system_list(connection: &rusqlite::Connection, verbose: bool) -> Result<(), CliError> {
    log_verbose(verbose, "list registered systems");
    let systems = list_systems(connection).map_err(|source| CliError::SystemList { source })?;
    for system in systems {
        println!("{}", system.name);
    }
    Ok(())
}

fn run_system_show(
    connection: &rusqlite::Connection,
    verbose: bool,
    name: &str,
) -> Result<(), CliError> {
    log_verbose(verbose, format!("show system: {name}"));
    let details =
        show_system(connection, name).map_err(|source| CliError::SystemShow { source })?;
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
        println!("DEPLOYMENT\tTYPE\tRELEASE\tSOURCE\tSTATUS");
        for deployment in deployments {
            let source = deployment.source_revision.as_deref().unwrap_or("-");
            println!(
                "{}\t{:?}\t{}\t{}\t{:?}",
                deployment.id,
                deployment.deployment_type,
                deployment.image_digest,
                source,
                deployment.status
            );
        }
    }
    Ok(())
}

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
    let application = resolve_application(connection, application_name)?;
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
    let deployed = deploy_oci(
        connection,
        &application.id,
        image_reference,
        None,
        Some(&public_configuration),
    )
    .map_err(|source| CliError::DeployOci {
        source: Box::new(source),
    })?;
    println!("Deployed {}", application.name);
    println!("Image: {}", deployed.image_reference);
    println!("Deployment: {}", deployed.deployment_id);
    println!("Runtime: {}", deployed.runtime_id);
    println!("Container: {}", deployed.container_name);
    println!("Status: Succeeded");
    Ok(())
}

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
    let deployed = deploy_branch(
        connection,
        &application.id,
        Some(branch),
        Some(&public_configuration),
    )
    .map_err(|source| CliError::DeployBranch {
        source: Box::new(source),
    })?;
    println!("Deployed {}", application.name);
    println!("Image: {}", deployed.image_reference);
    if let Some(source_revision) = &deployed.source_revision {
        println!("Source revision: {source_revision}");
    }
    println!("Deployment: {}", deployed.deployment_id);
    println!("Runtime: {}", deployed.runtime_id);
    println!("Container: {}", deployed.container_name);
    println!("Status: Succeeded");
    Ok(())
}

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
    println!("Image: {}", rolled_back.image_reference);
    if let Some(source_revision) = rolled_back.source_revision {
        println!("Source revision: {source_revision}");
    }
    println!("Deployment: {}", rolled_back.deployment_id);
    println!("Runtime: {}", rolled_back.runtime_id);
    println!("Status: Succeeded");
    Ok(())
}

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

fn configured_path(variable: &str, default: &str) -> PathBuf {
    env::var_os(variable)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn resolve_application(
    connection: &rusqlite::Connection,
    application_name: &str,
) -> Result<Application, CliError> {
    find_application_by_name(connection, application_name)
        .map_err(|source| CliError::List { source })?
        .ok_or_else(|| CliError::ApplicationNotFound {
            application_name: application_name.to_owned(),
        })
}

fn run_version() -> Result<(), CliError> {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

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
            let database_path = env::var_os(DATABASE_PATH_ENVIRONMENT_VARIABLE)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_DATABASE_PATH));

            let mut connection =
                database::open(&database_path).map_err(|source| CliError::Database { source })?;

            run_deploy_branch(&mut connection, verbose, &application, &branch)
        }
    }
}

fn run_doctor(connection: &rusqlite::Connection, verbose: bool) -> Result<(), CliError> {
    let mut all_ok = true;

    log_verbose(verbose, "checking database connection");
    match connection.query_row("SELECT 1", [], |_| Ok(())) {
        Ok(()) => println!("✓ Database connection: OK"),
        Err(source) => {
            println!("✗ Database connection: FAILED ({source})");
            all_ok = false;
        }
    }

    log_verbose(verbose, "checking database migrations");
    match connection.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
        row.get::<_, i64>(0)
    }) {
        Ok(count) => println!("✓ Database migrations: {count} applied"),
        Err(source) => {
            println!("✗ Database migrations: FAILED ({source})");
            all_ok = false;
        }
    }

    log_verbose(verbose, "checking workspace directory");
    let workspace_path = env::var_os(WORKSPACE_PATH_ENVIRONMENT_VARIABLE)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_PATH));
    if workspace_path.exists() {
        println!(
            "✓ Workspace directory: {} (exists)",
            workspace_path.display()
        );
    } else {
        println!(
            "✗ Workspace directory: {} (does not exist)",
            workspace_path.display()
        );
        all_ok = false;
    }

    log_verbose(verbose, "checking Caddy managed directory");
    let caddy_managed_path = env::var_os(CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CADDY_MANAGED_PATH));
    if caddy_managed_path.exists() {
        println!(
            "✓ Caddy managed directory: {} (exists)",
            caddy_managed_path.display()
        );
    } else {
        println!(
            "✗ Caddy managed directory: {} (does not exist)",
            caddy_managed_path.display()
        );
        all_ok = false;
    }

    log_verbose(verbose, "checking Caddyfile");
    let caddyfile_path = env::var_os(CADDYFILE_PATH_ENVIRONMENT_VARIABLE)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CADDYFILE_PATH));
    if caddyfile_path.exists() {
        println!("✓ Caddyfile: {} (exists)", caddyfile_path.display());
        match std::process::Command::new("caddy")
            .args(["validate", "--config"])
            .arg(&caddyfile_path)
            .args(["--adapter", "caddyfile"])
            .output()
        {
            Ok(output) if output.status.success() => println!("✓ Caddy configuration: valid"),
            Ok(output) => {
                println!(
                    "✗ Caddy configuration: FAILED ({})",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                all_ok = false;
            }
            Err(source) => {
                println!("✗ Caddy configuration: FAILED ({source})");
                all_ok = false;
            }
        }
    } else {
        println!("✗ Caddyfile: {} (does not exist)", caddyfile_path.display());
        all_ok = false;
    }

    log_verbose(verbose, "checking Git availability");
    match std::process::Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("✓ Git: {version}");
        }
        Ok(_) => {
            println!("✗ Git: command failed");
            all_ok = false;
        }
        Err(source) => {
            println!("✗ Git: not found ({source})");
            all_ok = false;
        }
    }

    log_verbose(verbose, "checking Podman availability");
    match std::process::Command::new("podman")
        .arg("--version")
        .output()
    {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("✓ Podman: {version}");
        }
        Ok(_) => {
            println!("✗ Podman: command failed");
            all_ok = false;
        }
        Err(source) => {
            println!("✗ Podman: not found ({source})");
            all_ok = false;
        }
    }

    match connection.prepare(
        "SELECT releases.image_reference
         FROM applications
         JOIN deployments ON deployments.id = applications.active_deployment_id
         JOIN releases ON releases.id = deployments.release_id",
    ) {
        Ok(mut statement) => match statement.query_map([], |row| row.get::<_, String>(0)) {
            Ok(images) => {
                for image in images {
                    match image {
                        Ok(image) if OciImageReference::parse(&image).is_ok() => {
                            match pull_image(&image) {
                                Ok(_) => println!("✓ Active OCI image: {image} (pullable)"),
                                Err(source) => {
                                    println!("✗ Active OCI image: {image} (FAILED: {source})");
                                    all_ok = false;
                                }
                            }
                        }
                        Ok(_) => println!("- Active local image: skipped"),
                        Err(source) => {
                            println!("✗ Active OCI image: FAILED ({source})");
                            all_ok = false;
                        }
                    }
                }
            }
            Err(source) => {
                println!("✗ Active OCI images: FAILED ({source})");
                all_ok = false;
            }
        },
        Err(source) => {
            println!("✗ Active OCI images: FAILED ({source})");
            all_ok = false;
        }
    }

    let database_path = configured_path(DATABASE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_DATABASE_PATH);
    for path in [&database_path, &workspace_path] {
        match std::process::Command::new("df")
            .args(["-Pk"])
            .arg(path)
            .output()
        {
            Ok(output) if output.status.success() => {
                let free_kib = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .nth(1)
                    .and_then(|line| line.split_whitespace().nth(3))
                    .and_then(|value| value.parse::<u64>().ok());
                if free_kib.is_some_and(|value| value >= 1024 * 1024) {
                    println!("✓ Disk space: {} (at least 1 GiB free)", path.display());
                } else {
                    println!("✗ Disk space: {} (less than 1 GiB free)", path.display());
                    all_ok = false;
                }
            }
            Ok(_) | Err(_) => {
                println!("✗ Disk space: {} (unable to inspect)", path.display());
                all_ok = false;
            }
        }
    }

    match std::process::Command::new("podman")
        .args(["info", "--format", "{{.Host.Security.Rootless}}"])
        .output()
    {
        Ok(output)
            if output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == "true" =>
        {
            println!("✓ Podman rootless: OK")
        }
        Ok(output) => {
            println!(
                "✗ Podman rootless: FAILED ({})",
                String::from_utf8_lossy(&output.stdout).trim()
            );
            all_ok = false;
        }
        Err(source) => {
            println!("✗ Podman rootless: FAILED ({source})");
            all_ok = false;
        }
    }

    log_verbose(verbose, "checking Podman Quadlet user generator");
    const QUADLET_GENERATOR_CANDIDATES: &[&str] = &[
        "/usr/lib/systemd/user-generators/podman-user-generator",
        "/lib/systemd/user-generators/podman-user-generator",
    ];
    let quadlet_generator = QUADLET_GENERATOR_CANDIDATES
        .iter()
        .find(|path| std::path::Path::new(path).is_file());
    if let Some(generator) = quadlet_generator {
        println!("✓ Podman Quadlet user generator: {generator}");
    } else {
        println!("✗ Podman Quadlet user generator: not found (install Podman >= 4.4 or Debian 13)");
        all_ok = false;
    }

    log_verbose(verbose, "checking Caddy availability");
    match std::process::Command::new("caddy").arg("version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("✓ Caddy: {version}");
        }
        Ok(_) => {
            println!("✗ Caddy: command failed");
            all_ok = false;
        }
        Err(source) => {
            println!("✗ Caddy: not found ({source})");
            all_ok = false;
        }
    }

    if all_ok {
        println!("\nAll checks passed!");
        Ok(())
    } else {
        println!("\nSome checks failed. Please review the output above.");
        Err(CliError::Doctor)
    }
}
