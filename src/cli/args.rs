use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use pneuma::control::Command as ControlCommand;
use pneuma::domain::exposure::Visibility;

use super::error::CliError;

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
    /// Open the interactive terminal interface
    Tui,
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

impl TryFrom<Commands> for InvocationTarget {
    type Error = CliError;

    fn try_from(cmd: Commands) -> Result<Self, Self::Error> {
        Ok(match cmd {
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
                    (Some(image_reference), None) => Self::Control(ControlCommand::DeployImage {
                        application_name,
                        image_reference,
                    }),
                    (None, Some(branch)) => Self::Control(ControlCommand::DeployBranch {
                        application_name,
                        branch,
                    }),
                    (None, None) => return Err(CliError::MissingDeployOption),
                    // Clap enforces the --image/--branch conflict before normalization runs.
                    (Some(_), Some(_)) => {
                        unreachable!("clap rejects --image and --branch as conflicting options")
                    }
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
            Commands::Tui => Self::Tui,
            Commands::Ci {
                command: CiCommands::Dispatch,
            } => Self::CiDispatch,
        })
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
    Tui,
    CiDispatch,
}

// Parses process arguments into the normalized invocation consumed by dispatch.
pub(crate) fn parse_invocation() -> Result<Invocation, CliError> {
    let cli = Cli::parse();
    Ok(Invocation {
        verbose: cli.verbose,
        target: InvocationTarget::try_from(cli.command)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // try_parse_from keeps the argument grammar under test without exiting the process.
    fn parse(arguments: &[&str]) -> Result<InvocationTarget, CliError> {
        let cli = Cli::try_parse_from(std::iter::once("pneuma").chain(arguments.iter().copied()))
            .expect("tested grammar must parse");
        InvocationTarget::try_from(cli.command)
    }

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
                InvocationTarget::try_from(parsed).expect("command input must normalize"),
                InvocationTarget::Control(expected)
            );
        }
    }

    #[test]
    fn image_and_branch_deploy_options_map_directly_to_control() {
        let image = InvocationTarget::try_from(Commands::App {
            command: AppCommands::Deploy {
                application_name: "portal".to_owned(),
                image: Some("registry.example/portal@sha256:abc".to_owned()),
                branch: None,
            },
        })
        .expect("image deploy input must normalize");
        assert_eq!(
            image,
            InvocationTarget::Control(ControlCommand::DeployImage {
                application_name: "portal".to_owned(),
                image_reference: "registry.example/portal@sha256:abc".to_owned(),
            })
        );

        let branch = InvocationTarget::try_from(Commands::App {
            command: AppCommands::Deploy {
                application_name: "portal".to_owned(),
                image: None,
                branch: Some("main".to_owned()),
            },
        })
        .expect("branch deploy input must normalize");
        assert_eq!(
            branch,
            InvocationTarget::Control(ControlCommand::DeployBranch {
                application_name: "portal".to_owned(),
                branch: "main".to_owned(),
            })
        );
    }

    #[test]
    fn grammar_normalizes_a_verbose_image_deploy() {
        let reference = "registry.example/portal@sha256:abc";
        let target = parse(&["--verbose", "app", "deploy", "portal", "--image", reference])
            .expect("image deploy input must normalize");

        assert_eq!(
            target,
            InvocationTarget::Control(ControlCommand::DeployImage {
                application_name: "portal".to_owned(),
                image_reference: reference.to_owned(),
            })
        );
    }

    #[test]
    fn grammar_normalizes_a_branch_deploy() {
        let target = parse(&["app", "deploy", "portal", "--branch", "staging"])
            .expect("branch deploy input must normalize");

        assert_eq!(
            target,
            InvocationTarget::Control(ControlCommand::DeployBranch {
                application_name: "portal".to_owned(),
                branch: "staging".to_owned(),
            })
        );
    }

    #[test]
    fn grammar_rejects_a_deploy_without_a_source_before_dispatch() {
        let error = parse(&["app", "deploy", "portal"])
            .expect_err("missing deploy source must fail normalization");

        assert!(matches!(error, CliError::MissingDeployOption));
        assert_eq!(
            error.to_string(),
            "either --image or --branch must be specified"
        );
    }

    #[test]
    fn grammar_rejects_mutually_exclusive_deploy_sources() {
        let error = Cli::try_parse_from([
            "pneuma",
            "app",
            "deploy",
            "portal",
            "--image",
            "registry.example/portal@sha256:abc",
            "--branch",
            "staging",
        ])
        .err()
        .expect("conflicting deploy sources must be rejected by clap");

        assert!(
            error.to_string().contains("cannot be used with"),
            "unexpected clap error: {error}"
        );
    }

    #[test]
    fn version_and_ci_remain_adapter_only_targets() {
        assert_eq!(
            InvocationTarget::try_from(Commands::Version).expect("version input must normalize"),
            InvocationTarget::Version
        );
        assert_eq!(
            InvocationTarget::try_from(Commands::Tui).expect("TUI input must normalize"),
            InvocationTarget::Tui
        );
        assert_eq!(
            InvocationTarget::try_from(Commands::Ci {
                command: CiCommands::Dispatch,
            })
            .expect("ci dispatch input must normalize"),
            InvocationTarget::CiDispatch
        );
    }
}
