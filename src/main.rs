use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use pneuma::database::{self, DatabaseError};
use pneuma::import_application::{ImportError, import_application};
use pneuma::list_applications::{ListError, list_applications};

const DATABASE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_DATABASE_PATH";
const DEFAULT_DATABASE_PATH: &str = "/var/lib/pneuma/database/pneuma.sqlite3";
const USAGE: &str = "Usage:\n  pneuma app import <repository-path>\n  pneuma app list";

enum Command {
    Import { repository_path: PathBuf },
    List,
}

#[derive(Debug)]
enum CliError {
    Usage,
    Database { source: DatabaseError },
    Import { source: ImportError },
    List { source: ListError },
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => formatter.write_str(USAGE),
            Self::Database { source } => write!(formatter, "{source}"),
            Self::Import { source } => write!(formatter, "{source}"),
            Self::List { source } => write!(formatter, "{source}"),
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

fn parse_command(arguments: &[OsString]) -> Result<Command, CliError> {
    match arguments {
        [app, import, repository_path]
            if app == OsStr::new("app") && import == OsStr::new("import") =>
        {
            Ok(Command::Import {
                repository_path: PathBuf::from(repository_path),
            })
        }
        [app, list] if app == OsStr::new("app") && list == OsStr::new("list") => Ok(Command::List),
        _ => Err(CliError::Usage),
    }
}

fn run(command: Command) -> Result<(), CliError> {
    let database_path = env::var_os(DATABASE_PATH_ENVIRONMENT_VARIABLE)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATABASE_PATH));
    let mut connection =
        database::open(&database_path).map_err(|source| CliError::Database { source })?;

    match command {
        Command::Import { repository_path } => {
            let application = import_application(&mut connection, &repository_path)
                .map_err(|source| CliError::Import { source })?;
            println!("Imported {}", application.name);
            println!("Status: Registered");
            println!("Deployment: Not deployed");
        }
        Command::List => {
            let applications =
                list_applications(&connection).map_err(|source| CliError::List { source })?;
            for application in applications {
                println!("{}\tRegistered\tNot deployed", application.name);
            }
        }
    }

    Ok(())
}
