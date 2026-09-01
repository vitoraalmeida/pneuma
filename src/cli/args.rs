use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use pneuma::control::Command as ControlCommand;
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

impl From<Commands> for InvocationTarget {
    fn from(cmd: Commands) -> Self {
        match cmd {
            Commands::System { command } => match command {
                SystemCommands::Create { name, description } => {
                    Self::Control(ControlCommand::SystemCreate { name, description })
                }
                SystemCommands::List => Self::Control(ControlCommand::SystemList),
                SystemCommands::Show { name } => Self::Control(ControlCommand::SystemShow { name }),
            },
            Commands::App { command } => match command {
                AppCommands::Import {
                    repository,
                    system,
                    manifest,
                } => Self::Control(ControlCommand::ImportApplication {
                    repository,
                    system_name: system,
                    manifest_path: manifest,
                }),
                AppCommands::List => Self::Control(ControlCommand::ListApplications),
                AppCommands::Deployments { application_name } => {
                    Self::Control(ControlCommand::ListDeployments { application_name })
                }
                AppCommands::Status { application_name } => {
                    Self::Control(ControlCommand::ApplicationStatus { application_name })
                }
                AppCommands::Stop { application_name } => {
                    Self::Control(ControlCommand::ApplicationStop { application_name })
                }
                AppCommands::Start { application_name } => {
                    Self::Control(ControlCommand::ApplicationStart { application_name })
                }
                AppCommands::Deploy {
                    application_name,
                    image,
                    branch,
                } => match (image, branch) {
                    (_, Some(branch)) => Self::Control(ControlCommand::DeployBranch {
                        application_name,
                        branch,
                    }),
                    (Some(image_reference), None) => Self::Control(ControlCommand::DeployImage {
                        application_name,
                        image_reference,
                    }),
                    (None, None) => Self::MissingDeployOption,
                },
                AppCommands::Visibility { command } => match command {
                    VisibilityCommands::Set {
                        application_name,
                        visibility,
                    } => Self::Control(ControlCommand::VisibilitySet {
                        application_name,
                        visibility: visibility.into(),
                    }),
                },
            },
            Commands::Deployment { command } => match command {
                DeploymentCommands::Rollback { application_name } => {
                    Self::Control(ControlCommand::Rollback { application_name })
                }
            },
            Commands::Database { command } => match command {
                DatabaseCommands::Backup { path } => {
                    Self::Control(ControlCommand::DatabaseBackup { path })
                }
                DatabaseCommands::Restore { path } => {
                    Self::Control(ControlCommand::DatabaseRestore { path })
                }
            },
            Commands::Version => Self::Version,
            Commands::Doctor => Self::Control(ControlCommand::Doctor),
            Commands::Reconcile { application_name } => {
                Self::Control(ControlCommand::Reconcile { application_name })
            }
            Commands::Ci { .. } => Self::CiDispatch,
        }
    }
}

// Keeps global CLI settings alongside its execution target for dispatch.
pub(crate) struct Invocation {
    pub(crate) verbose: bool,
    pub(crate) target: InvocationTarget,
}

// Identifies the adapter-only paths and wraps every command sent to control.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InvocationTarget {
    Control(ControlCommand),
    Version,
    CiDispatch,
    MissingDeployOption,
}

// Parses process arguments into the normalized invocation consumed by dispatch.
pub(crate) fn parse_invocation() -> Invocation {
    let cli = Cli::parse();
    Invocation {
        verbose: cli.verbose,
        target: cli.command.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_commands_map_directly_to_control() {
        let cases = [
            (
                Commands::System {
                    command: SystemCommands::Create {
                        name: "platform".to_owned(),
                        description: Some("Platform services".to_owned()),
                    },
                },
                ControlCommand::SystemCreate {
                    name: "platform".to_owned(),
                    description: Some("Platform services".to_owned()),
                },
            ),
            (
                Commands::App {
                    command: AppCommands::Import {
                        repository: "https://git.example/portal.git".to_owned(),
                        system: Some("platform".to_owned()),
                        manifest: Some("deploy/pneuma.toml".to_owned()),
                    },
                },
                ControlCommand::ImportApplication {
                    repository: "https://git.example/portal.git".to_owned(),
                    system_name: Some("platform".to_owned()),
                    manifest_path: Some("deploy/pneuma.toml".to_owned()),
                },
            ),
            (
                Commands::App {
                    command: AppCommands::Visibility {
                        command: VisibilityCommands::Set {
                            application_name: "portal".to_owned(),
                            visibility: VisibilityArg::Public,
                        },
                    },
                },
                ControlCommand::VisibilitySet {
                    application_name: "portal".to_owned(),
                    visibility: Visibility::Public,
                },
            ),
            (
                Commands::Database {
                    command: DatabaseCommands::Restore {
                        path: PathBuf::from("backup.sqlite3"),
                    },
                },
                ControlCommand::DatabaseRestore {
                    path: PathBuf::from("backup.sqlite3"),
                },
            ),
            (
                Commands::Reconcile {
                    application_name: "portal".to_owned(),
                },
                ControlCommand::Reconcile {
                    application_name: "portal".to_owned(),
                },
            ),
        ];

        for (parsed, expected) in cases {
            assert_eq!(
                InvocationTarget::from(parsed),
                InvocationTarget::Control(expected)
            );
        }
    }

    #[test]
    fn deploy_options_select_a_control_command_or_the_existing_usage_error() {
        let image = InvocationTarget::from(Commands::App {
            command: AppCommands::Deploy {
                application_name: "portal".to_owned(),
                image: Some("registry.example/portal@sha256:abc".to_owned()),
                branch: None,
            },
        });
        assert_eq!(
            image,
            InvocationTarget::Control(ControlCommand::DeployImage {
                application_name: "portal".to_owned(),
                image_reference: "registry.example/portal@sha256:abc".to_owned(),
            })
        );

        let branch = InvocationTarget::from(Commands::App {
            command: AppCommands::Deploy {
                application_name: "portal".to_owned(),
                image: None,
                branch: Some("main".to_owned()),
            },
        });
        assert_eq!(
            branch,
            InvocationTarget::Control(ControlCommand::DeployBranch {
                application_name: "portal".to_owned(),
                branch: "main".to_owned(),
            })
        );

        let missing = InvocationTarget::from(Commands::App {
            command: AppCommands::Deploy {
                application_name: "portal".to_owned(),
                image: None,
                branch: None,
            },
        });
        assert_eq!(missing, InvocationTarget::MissingDeployOption);
    }

    #[test]
    fn version_and_ci_remain_adapter_only_targets() {
        assert_eq!(
            InvocationTarget::from(Commands::Version),
            InvocationTarget::Version
        );
        assert_eq!(
            InvocationTarget::from(Commands::Ci {
                command: CiCommands::Dispatch,
            }),
            InvocationTarget::CiDispatch
        );
    }
}
