use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pneuma::adapters::database::{self, DatabaseError};
use pneuma::adapters::oci_image::{OciImageReference, pull_image};
use pneuma::domain::application::Application;
use pneuma::domain::manifest::Visibility;
use pneuma::use_cases::application_import::{ImportError, import_application};
use pneuma::use_cases::application_list::{ListError, application_is_deployed, list_applications};
use pneuma::use_cases::application_runtime::{
    RuntimeLifecycleError, report_application_status, start_application, stop_application,
};
use pneuma::use_cases::deployment_deploy_oci::{DeployOciError, deploy_oci};
use pneuma::use_cases::deployment_deploy_release::PublicDeploymentConfiguration;
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
const USAGE: &str = "Usage:\n  pneuma [--verbose] system create <name> [--description <text>]\n  pneuma [--verbose] system list\n  pneuma [--verbose] system show <name>\n  pneuma [--verbose] app import <repository-path> [--system <system-name>]\n  pneuma [--verbose] app list\n  pneuma [--verbose] app deployments <application-name>\n  pneuma [--verbose] app status <application-name>\n  pneuma [--verbose] app stop <application-name>\n  pneuma [--verbose] app start <application-name>\n  pneuma [--verbose] app deploy <application-name> --image <repository@sha256:...>\n  pneuma [--verbose] deployment rollback <application-name>\n  pneuma [--verbose] app visibility set <application-name> <public|internal>\n  pneuma database backup <path>\n  pneuma database restore <path>\n  pneuma version\n  pneuma doctor";

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
        repository_path: PathBuf,
        system_name: Option<String>,
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
        image_reference: String,
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
}

#[derive(Debug)]
enum CliError {
    Usage,
    Database {
        source: DatabaseError,
    },
    Import {
        source: ImportError,
    },
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
    Doctor,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => formatter.write_str(USAGE),
            Self::Database { source } => write!(formatter, "{source}"),
            Self::Import { source } => write!(formatter, "{source}"),
            Self::List { source } => write!(formatter, "{source}"),
            Self::ListDeployments { source } => write!(formatter, "{source}"),
            Self::ApplicationNotFound { application_name } => {
                write!(formatter, "application `{application_name}` was not found")
            }
            Self::ApplicationRuntime { source } => write!(formatter, "{source}"),
            Self::DeployOci { source } => write!(formatter, "{source}"),
            Self::Rollback { source } => write!(formatter, "{source}"),
            Self::VisibilitySet { source } => write!(formatter, "{source}"),
            Self::DatabaseBackup { source } | Self::DatabaseRestore { source } => {
                write!(formatter, "{source}")
            }
            Self::SystemCreate { source } => write!(formatter, "{source}"),
            Self::SystemList { source } => write!(formatter, "{source}"),
            Self::SystemShow { source } => write!(formatter, "{source}"),
            Self::Doctor => formatter.write_str("one or more diagnostic checks failed"),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Usage => None,
            Self::Database { source } => Some(source),
            Self::Import { source } => Some(source),
            Self::List { source } => Some(source),
            Self::ListDeployments { source } => Some(source),
            Self::DeployOci { source } => Some(source.as_ref()),
            Self::ApplicationRuntime { source } => Some(source.as_ref()),
            Self::ApplicationNotFound { .. } => None,
            Self::Rollback { source } => Some(source),
            Self::VisibilitySet { source } => Some(source),
            Self::DatabaseBackup { source } | Self::DatabaseRestore { source } => Some(source),
            Self::SystemCreate { source } => Some(source),
            Self::SystemList { source } => Some(source),
            Self::SystemShow { source } => Some(source),
            Self::Doctor => None,
        }
    }
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    let result = parse_command(&arguments).and_then(run);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_command(arguments: &[OsString]) -> Result<Invocation, CliError> {
    let (verbose, arguments) = match arguments {
        [verbose, remaining @ ..] if verbose == OsStr::new("--verbose") => (true, remaining),
        arguments => (false, arguments),
    };
    let command = match arguments {
        [system, create, name]
            if system == OsStr::new("system") && create == OsStr::new("create") =>
        {
            let name = name.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::SystemCreate {
                name,
                description: None,
            })
        }
        [system, create, name, description_flag, description]
            if system == OsStr::new("system")
                && create == OsStr::new("create")
                && description_flag == OsStr::new("--description") =>
        {
            let name = name.to_str().ok_or(CliError::Usage)?.to_owned();
            let description = description.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::SystemCreate {
                name,
                description: Some(description),
            })
        }
        [system, list] if system == OsStr::new("system") && list == OsStr::new("list") => {
            Ok(Command::SystemList)
        }
        [system, show, name] if system == OsStr::new("system") && show == OsStr::new("show") => {
            let name = name.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::SystemShow { name })
        }
        [app, import, repository_path]
            if app == OsStr::new("app") && import == OsStr::new("import") =>
        {
            Ok(Command::Import {
                repository_path: PathBuf::from(repository_path),
                system_name: None,
            })
        }
        [app, import, repository_path, system_flag, system_name]
            if app == OsStr::new("app")
                && import == OsStr::new("import")
                && system_flag == OsStr::new("--system") =>
        {
            let system_name = system_name.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::Import {
                repository_path: PathBuf::from(repository_path),
                system_name: Some(system_name),
            })
        }
        [app, list] if app == OsStr::new("app") && list == OsStr::new("list") => Ok(Command::List),
        [app, deployments, application_name]
            if app == OsStr::new("app") && deployments == OsStr::new("deployments") =>
        {
            let application_name = application_name.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::Deployments { application_name })
        }
        [app, status, application_name]
            if app == OsStr::new("app") && status == OsStr::new("status") =>
        {
            let application_name = application_name.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::Status { application_name })
        }
        [app, stop, application_name] if app == OsStr::new("app") && stop == OsStr::new("stop") => {
            let application_name = application_name.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::Stop { application_name })
        }
        [app, start, application_name]
            if app == OsStr::new("app") && start == OsStr::new("start") =>
        {
            let application_name = application_name.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::Start { application_name })
        }
        [app, deploy, application_name, image_option, image_reference]
            if app == OsStr::new("app")
                && deploy == OsStr::new("deploy")
                && image_option == OsStr::new("--image") =>
        {
            let application_name = application_name.to_str().ok_or(CliError::Usage)?.to_owned();
            let image_reference = image_reference.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::Deploy {
                application_name,
                image_reference,
            })
        }
        [deployment, rollback, application_name]
            if deployment == OsStr::new("deployment") && rollback == OsStr::new("rollback") =>
        {
            let application_name = application_name.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::Rollback { application_name })
        }
        [
            app,
            visibility_command,
            set,
            application_name,
            visibility_value,
        ] if app == OsStr::new("app")
            && visibility_command == OsStr::new("visibility")
            && set == OsStr::new("set") =>
        {
            let application_name = application_name.to_str().ok_or(CliError::Usage)?.to_owned();
            let visibility_str = visibility_value.to_str().ok_or(CliError::Usage)?;
            let visibility = match visibility_str {
                "public" => Visibility::Public,
                "internal" => Visibility::Internal,
                _ => return Err(CliError::Usage),
            };
            Ok(Command::VisibilitySet {
                application_name,
                visibility,
            })
        }
        [version] if version == OsStr::new("version") => Ok(Command::Version),
        [doctor] if doctor == OsStr::new("doctor") => Ok(Command::Doctor),
        [database, backup, path]
            if database == OsStr::new("database") && backup == OsStr::new("backup") =>
        {
            Ok(Command::DatabaseBackup {
                path: PathBuf::from(path),
            })
        }
        [database, restore, path]
            if database == OsStr::new("database") && restore == OsStr::new("restore") =>
        {
            Ok(Command::DatabaseRestore {
                path: PathBuf::from(path),
            })
        }
        _ => Err(CliError::Usage),
    }?;
    Ok(Invocation { verbose, command })
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
            repository_path,
            system_name,
        } => run_import(
            &mut connection,
            verbose,
            &repository_path,
            system_name.as_deref(),
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
        } => run_deploy_oci(
            &mut connection,
            verbose,
            &application_name,
            &image_reference,
        ),
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
        | Command::DatabaseRestore { .. } => unreachable!(),
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
    repository_path: &Path,
    system_name: Option<&str>,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("import repository: {}", repository_path.display()),
    );
    let application = import_application(
        connection,
        repository_path,
        system_name,
        Some(repository_path.to_str().ok_or(CliError::Usage)?),
        Some("pneuma.toml"),
    )
    .map_err(|source| CliError::Import { source })?;
    println!("Imported {}", application.name);
    println!("Status: Registered");
    println!("Deployment: Not deployed");
    Ok(())
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
        println!("DEPLOYMENT\tRELEASE\tSOURCE\tSTATUS");
        for deployment in deployments {
            let source = deployment.source_revision.as_deref().unwrap_or("-");
            println!(
                "{}\t{}\t{}\t{:?}",
                deployment.id, deployment.image_digest, source, deployment.status
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
    list_applications(connection)
        .map_err(|source| CliError::List { source })?
        .into_iter()
        .find(|application| application.name == application_name)
        .ok_or_else(|| CliError::ApplicationNotFound {
            application_name: application_name.to_owned(),
        })
}

fn run_version() -> Result<(), CliError> {
    println!("pneuma {}", env!("CARGO_PKG_VERSION"));
    Ok(())
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
