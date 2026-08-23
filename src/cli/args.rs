use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use pneuma::domain::exposure::Visibility;

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
pub(crate) struct Invocation {
    pub(crate) verbose: bool,
    pub(crate) command: Command,
}

pub(crate) enum Command {
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

// Parses process arguments into the normalized invocation consumed by dispatch.
pub(crate) fn parse_invocation() -> Invocation {
    let cli = Cli::parse();
    Invocation {
        verbose: cli.verbose,
        command: cli.command.into(),
    }
}
