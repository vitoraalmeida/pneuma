use std::error::Error;
use std::fmt;
use std::io;
use std::process::Command;

const APPLICATION_LABEL: &str = "io.pneuma.application";
const REVISION_LABEL: &str = "io.pneuma.revision";
const ROLE_LABEL: &str = "io.pneuma.role";

#[derive(Debug, PartialEq, Eq)]
pub struct CreatedContainer {
    pub id: String,
    pub name: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub enum CreateContainerError {
    InvalidPort,
    Execute {
        source: io::Error,
    },
    Create {
        name: String,
        stdout: String,
        stderr: String,
    },
    InvalidOutput {
        name: String,
    },
}

impl fmt::Display for CreateContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPort => formatter.write_str("container port must be between 1 and 65535"),
            Self::Execute { source } => write!(formatter, "failed to execute Podman: {source}"),
            Self::Create {
                name,
                stdout,
                stderr,
            } => {
                let diagnostic = if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                };
                write!(
                    formatter,
                    "failed to create container `{name}` with Podman: {diagnostic}"
                )
            }
            Self::InvalidOutput { name } => write!(
                formatter,
                "Podman returned an invalid ID for created container `{name}`"
            ),
        }
    }
}

impl Error for CreateContainerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source } => Some(source),
            Self::InvalidPort | Self::Create { .. } | Self::InvalidOutput { .. } => None,
        }
    }
}

pub fn create_container(
    image_reference: &str,
    application_name: &str,
    commit_sha: &str,
    container_port: u16,
) -> Result<CreatedContainer, CreateContainerError> {
    if container_port == 0 {
        return Err(CreateContainerError::InvalidPort);
    }

    let name = container_name(application_name, commit_sha);
    let application_label = format!("{APPLICATION_LABEL}={application_name}");
    let revision_label = format!("{REVISION_LABEL}={commit_sha}");
    let role_label = format!("{ROLE_LABEL}=candidate");
    // Let Podman choose an unused host port, but constrain the mapping to loopback so a
    // candidate cannot become publicly reachable before health checks and promotion.
    let port_mapping = format!("127.0.0.1::{container_port}");
    let output = Command::new("podman")
        .args(["create", "--pull=never", "--name"])
        .arg(&name)
        .arg("--label")
        .arg(application_label)
        .arg("--label")
        .arg(revision_label)
        .arg("--label")
        .arg(role_label)
        .arg("--publish")
        .arg(port_mapping)
        .arg(image_reference)
        .output()
        .map_err(|source| CreateContainerError::Execute { source })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        return Err(CreateContainerError::Create {
            name,
            stdout,
            stderr,
        });
    }

    let id = stdout.trim();
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CreateContainerError::InvalidOutput { name });
    }

    Ok(CreatedContainer {
        id: id.to_owned(),
        name,
        stdout,
        stderr,
    })
}

fn container_name(application_name: &str, commit_sha: &str) -> String {
    format!("pneuma-{application_name}-{commit_sha}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_identity_is_determined_by_application_and_commit() {
        let commit_sha = "e".repeat(40);

        assert_eq!(
            container_name("personal-site", &commit_sha),
            format!("pneuma-personal-site-{commit_sha}")
        );
    }

    #[test]
    fn rejects_port_zero_before_running_podman() {
        let error = create_container("image", "personal-site", "e48c715", 0).unwrap_err();

        assert!(matches!(error, CreateContainerError::InvalidPort));
    }
}
