use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use pneuma::database::{self, DatabaseError};
use pneuma::deploy_internal_revision::{
    DeployInternalRevisionError, deploy_internal_revision, deploy_internal_revision_with_progress,
};
use pneuma::import_application::{ImportError, import_application};
use pneuma::list_applications::{ListError, list_applications};

const DATABASE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_DATABASE_PATH";
const DEFAULT_DATABASE_PATH: &str = "/var/lib/pneuma/database/pneuma.sqlite3";
const WORKSPACE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_WORKSPACE_PATH";
const DEFAULT_WORKSPACE_PATH: &str = "/var/lib/pneuma/checkouts";
const USAGE: &str = "Usage:\n  pneuma [--verbose] app import <repository-path>\n  pneuma [--verbose] app list\n  pneuma [--verbose] app deploy <application-name> <repository-path> --revision <revision>";

struct Invocation {
    verbose: bool,
    command: Command,
}

enum Command {
    Import {
        repository_path: PathBuf,
    },
    List,
    Deploy {
        application_name: String,
        repository_path: PathBuf,
        revision: String,
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
    ApplicationNotFound {
        application_name: String,
    },
    Deploy {
        source: Box<DeployInternalRevisionError>,
    },
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => formatter.write_str(USAGE),
            Self::Database { source } => write!(formatter, "{source}"),
            Self::Import { source } => write!(formatter, "{source}"),
            Self::List { source } => write!(formatter, "{source}"),
            Self::ApplicationNotFound { application_name } => {
                write!(formatter, "application `{application_name}` was not found")
            }
            Self::Deploy { source } => write!(formatter, "{source}"),
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
            Self::Deploy { source } => Some(source.as_ref()),
            Self::ApplicationNotFound { .. } => None,
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
        [app, import, repository_path]
            if app == OsStr::new("app") && import == OsStr::new("import") =>
        {
            Ok(Command::Import {
                repository_path: PathBuf::from(repository_path),
            })
        }
        [app, list] if app == OsStr::new("app") && list == OsStr::new("list") => Ok(Command::List),
        [
            app,
            deploy,
            application_name,
            repository_path,
            revision_option,
            revision,
        ] if app == OsStr::new("app")
            && deploy == OsStr::new("deploy")
            && revision_option == OsStr::new("--revision") =>
        {
            let application_name = application_name.to_str().ok_or(CliError::Usage)?.to_owned();
            let revision = revision.to_str().ok_or(CliError::Usage)?.to_owned();
            Ok(Command::Deploy {
                application_name,
                repository_path: PathBuf::from(repository_path),
                revision,
            })
        }
        _ => Err(CliError::Usage),
    }?;
    Ok(Invocation { verbose, command })
}

fn run(invocation: Invocation) -> Result<(), CliError> {
    let Invocation { verbose, command } = invocation;
    let database_path = env::var_os(DATABASE_PATH_ENVIRONMENT_VARIABLE)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATABASE_PATH));
    if verbose {
        eprintln!("[verbose] database: {}", database_path.display());
    }
    let mut connection =
        database::open(&database_path).map_err(|source| CliError::Database { source })?;

    match command {
        Command::Import { repository_path } => {
            if verbose {
                eprintln!("[verbose] import repository: {}", repository_path.display());
            }
            let application = import_application(&mut connection, &repository_path)
                .map_err(|source| CliError::Import { source })?;
            println!("Imported {}", application.name);
            println!("Status: Registered");
            println!("Deployment: Not deployed");
        }
        Command::List => {
            if verbose {
                eprintln!("[verbose] list registered applications");
            }
            let applications =
                list_applications(&connection).map_err(|source| CliError::List { source })?;
            for application in applications {
                println!("{}\tRegistered\tNot deployed", application.name);
            }
        }
        Command::Deploy {
            application_name,
            repository_path,
            revision,
        } => {
            if verbose {
                eprintln!("[verbose] resolve application by name: {application_name}");
            }
            let application = list_applications(&connection)
                .map_err(|source| CliError::List { source })?
                .into_iter()
                .find(|application| application.name == application_name)
                .ok_or_else(|| CliError::ApplicationNotFound {
                    application_name: application_name.clone(),
                })?;
            let workspace_path = env::var_os(WORKSPACE_PATH_ENVIRONMENT_VARIABLE)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_PATH));
            if verbose {
                eprintln!(
                    "[verbose] deployment input: application {}, repository {}, revision {}, workspace {}",
                    application.name,
                    repository_path.display(),
                    revision,
                    workspace_path.display()
                );
            } else {
                eprintln!("Deploying {}...", application.name);
            }
            let deployment = if verbose {
                let mut report_progress = |event| eprintln!("[verbose] {event}");
                deploy_internal_revision_with_progress(
                    &mut connection,
                    &application.id,
                    &repository_path,
                    &revision,
                    &workspace_path,
                    &mut report_progress,
                )
            } else {
                deploy_internal_revision(
                    &mut connection,
                    &application.id,
                    &repository_path,
                    &revision,
                    &workspace_path,
                )
            };
            let deployed = deployment.map_err(|source| CliError::Deploy {
                source: Box::new(source),
            })?;
            println!("Deployed {}", application.name);
            println!("Commit: {}", deployed.commit_sha);
            println!("Deployment: {}", deployed.deployment_id);
            println!("Runtime: {}", deployed.runtime_id);
            println!("Container: {}", deployed.container_name);
            println!("Status: Succeeded");
        }
    }

    Ok(())
}
