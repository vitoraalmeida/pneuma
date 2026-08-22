use std::error::Error;
use std::fmt;

use crate::domain::application::ApplicationName;

#[derive(Debug, PartialEq)]
pub enum CiCommand {
    Deploy { application: String, branch: String },
    Version,
}

#[derive(Debug)]
pub enum CiDispatchError {
    MissingSshOriginalCommand,
    EmptyCommand,
    UnknownCommand { command: String },
    InvalidApplicationName { name: String },
    InvalidBranchName { name: String },
    InvalidDeployFormat,
}

impl fmt::Display for CiDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSshOriginalCommand => {
                write!(formatter, "SSH_ORIGINAL_COMMAND not set")
            }
            Self::EmptyCommand => write!(formatter, "empty command"),
            Self::UnknownCommand { command } => {
                write!(formatter, "unknown command: {command}")
            }
            Self::InvalidApplicationName { name } => {
                write!(
                    formatter,
                    "invalid application name `{name}`: use 1-63 lowercase alphanumeric characters or hyphens"
                )
            }
            Self::InvalidBranchName { name } => {
                write!(
                    formatter,
                    "invalid branch name `{name}`: shell metacharacters are not allowed"
                )
            }
            Self::InvalidDeployFormat => {
                write!(
                    formatter,
                    "invalid deploy format: expected `deploy <application> <branch>`"
                )
            }
        }
    }
}

impl Error for CiDispatchError {}

// Rejects shell metacharacters because branch names cross the SSH command boundary.
fn is_valid_branch_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let shell_metacharacters = [
        ';', '&', '|', '$', '`', '(', ')', '{', '}', '<', '>', '!', '\\', '\'', '"', ' ', '\t',
        '\n', '\r',
    ];
    !name.chars().any(|c| shell_metacharacters.contains(&c))
}

// Parses the small SSH command protocol and validates every value before deployment dispatch.
pub fn parse_ci_command(input: &str) -> Result<CiCommand, CiDispatchError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(CiDispatchError::EmptyCommand);
    }

    let mut parts = input.split_whitespace();
    let command = parts.next().ok_or(CiDispatchError::EmptyCommand)?;

    match command {
        "version" => {
            if parts.next().is_some() {
                return Err(CiDispatchError::UnknownCommand {
                    command: input.to_owned(),
                });
            }
            Ok(CiCommand::Version)
        }
        "deploy" => {
            let application = parts
                .next()
                .ok_or(CiDispatchError::InvalidDeployFormat)?
                .to_owned();
            let branch = parts
                .next()
                .ok_or(CiDispatchError::InvalidDeployFormat)?
                .to_owned();

            if parts.next().is_some() {
                return Err(CiDispatchError::InvalidDeployFormat);
            }

            if ApplicationName::new(&application).is_err() {
                return Err(CiDispatchError::InvalidApplicationName { name: application });
            }

            if !is_valid_branch_name(&branch) {
                return Err(CiDispatchError::InvalidBranchName { name: branch });
            }

            Ok(CiCommand::Deploy {
                application,
                branch,
            })
        }
        _ => Err(CiDispatchError::UnknownCommand {
            command: command.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version() {
        assert_eq!(parse_ci_command("version").unwrap(), CiCommand::Version);
    }

    #[test]
    fn parse_version_with_extra_args_rejected() {
        assert!(parse_ci_command("version extra").is_err());
    }

    #[test]
    fn parse_deploy() {
        assert_eq!(
            parse_ci_command("deploy my-app staging").unwrap(),
            CiCommand::Deploy {
                application: "my-app".to_owned(),
                branch: "staging".to_owned(),
            }
        );
    }

    #[test]
    fn parse_deploy_reuses_the_domain_application_name_rule() {
        assert_eq!(
            parse_ci_command("deploy my-app main").unwrap(),
            CiCommand::Deploy {
                application: "my-app".to_owned(),
                branch: "main".to_owned(),
            }
        );
        // The dispatcher now rejects names that the catalog domain would reject.
        assert!(parse_ci_command("deploy my.app_v2 main").is_err());
    }

    #[test]
    fn parse_deploy_missing_branch_rejected() {
        assert!(parse_ci_command("deploy my-app").is_err());
    }

    #[test]
    fn parse_deploy_missing_application_rejected() {
        assert!(parse_ci_command("deploy").is_err());
    }

    #[test]
    fn parse_deploy_extra_args_rejected() {
        assert!(parse_ci_command("deploy my-app staging extra").is_err());
    }

    #[test]
    fn parse_deploy_invalid_application_name_rejected() {
        assert!(parse_ci_command("deploy my@app staging").is_err());
        assert!(parse_ci_command("deploy my app staging").is_err());
        assert!(parse_ci_command("deploy my;app staging").is_err());
        assert!(parse_ci_command("deploy my$(id) staging").is_err());
    }

    #[test]
    fn parse_empty_command_rejected() {
        assert!(parse_ci_command("").is_err());
        assert!(parse_ci_command("   ").is_err());
    }

    #[test]
    fn parse_unknown_command_rejected() {
        assert!(parse_ci_command("id").is_err());
        assert!(parse_ci_command("podman ps").is_err());
        assert!(parse_ci_command("cat /etc/passwd").is_err());
    }

    #[test]
    fn parse_injection_attempts_rejected() {
        assert!(parse_ci_command("deploy app main; id").is_err());
        assert!(parse_ci_command("deploy app $(id)").is_err());
        assert!(parse_ci_command("deploy app `id`").is_err());
        assert!(parse_ci_command("deploy app main && touch /tmp/x").is_err());
    }

    #[test]
    fn valid_application_names() {
        assert!(parse_ci_command("deploy my-app main").is_ok());
        assert!(parse_ci_command("deploy a main").is_ok());
        assert!(parse_ci_command("deploy myapp123 main").is_ok());
    }

    #[test]
    fn invalid_application_names() {
        assert!(parse_ci_command("deploy my app main").is_err());
        assert!(parse_ci_command("deploy my@app main").is_err());
        assert!(parse_ci_command("deploy my;app main").is_err());
        assert!(parse_ci_command("deploy my$(id) main").is_err());
        assert!(parse_ci_command("deploy my`id` main").is_err());
        assert!(parse_ci_command("deploy my.app main").is_err());
        assert!(parse_ci_command("deploy my_app main").is_err());
        assert!(parse_ci_command("deploy MyApp123 main").is_err());
    }
}
