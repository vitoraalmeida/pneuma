use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
pub struct BuiltImage {
    pub reference: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub enum BuildImageError {
    ResolvePath {
        field: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    OutsideCheckout {
        field: &'static str,
        path: PathBuf,
    },
    InvalidPathType {
        field: &'static str,
        path: PathBuf,
        expected: &'static str,
    },
    Execute {
        source: io::Error,
    },
    Build {
        reference: String,
        stdout: String,
        stderr: String,
    },
}

impl fmt::Display for BuildImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResolvePath {
                field,
                path,
                source,
            } => write!(
                formatter,
                "failed to resolve build {field} at {}: {source}",
                path.display()
            ),
            Self::OutsideCheckout { field, path } => write!(
                formatter,
                "build {field} escapes the checkout: {}",
                path.display()
            ),
            Self::InvalidPathType {
                field,
                path,
                expected,
            } => write!(
                formatter,
                "build {field} at {} must be {expected}",
                path.display()
            ),
            Self::Execute { source } => write!(formatter, "failed to execute Podman: {source}"),
            Self::Build {
                reference,
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
                    "failed to build image `{reference}` with Podman: {diagnostic}"
                )
            }
        }
    }
}

impl Error for BuildImageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ResolvePath { source, .. } | Self::Execute { source } => Some(source),
            Self::OutsideCheckout { .. } | Self::InvalidPathType { .. } | Self::Build { .. } => {
                None
            }
        }
    }
}

pub fn build_image(
    checkout_path: &Path,
    application_name: &str,
    commit_sha: &str,
    containerfile: &Path,
    context: &Path,
) -> Result<BuiltImage, BuildImageError> {
    let checkout_path = canonicalize("checkout", checkout_path)?;
    if !checkout_path.is_dir() {
        return Err(BuildImageError::InvalidPathType {
            field: "checkout",
            path: checkout_path,
            expected: "a directory",
        });
    }

    let containerfile = confined_path(&checkout_path, "containerfile", containerfile)?;
    if !containerfile.is_file() {
        return Err(BuildImageError::InvalidPathType {
            field: "containerfile",
            path: containerfile,
            expected: "a file",
        });
    }

    let context = confined_path(&checkout_path, "context", context)?;
    if !context.is_dir() {
        return Err(BuildImageError::InvalidPathType {
            field: "context",
            path: context,
            expected: "a directory",
        });
    }

    let reference = image_reference(application_name, commit_sha);
    let output = Command::new("podman")
        .args(["build", "--tag"])
        .arg(&reference)
        .arg("--file")
        .arg(containerfile)
        .arg(context)
        .output()
        .map_err(|source| BuildImageError::Execute { source })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        return Err(BuildImageError::Build {
            reference,
            stdout,
            stderr,
        });
    }

    Ok(BuiltImage {
        reference,
        stdout,
        stderr,
    })
}

fn confined_path(
    checkout_path: &Path,
    field: &'static str,
    configured_path: &Path,
) -> Result<PathBuf, BuildImageError> {
    // Compare resolved paths so `..` components and symlinks cannot disguise a build
    // input outside the controlled checkout and expose unrelated host files to Podman.
    let joined_path = checkout_path.join(configured_path);
    let resolved_path = canonicalize(field, &joined_path)?;
    if !resolved_path.starts_with(checkout_path) {
        return Err(BuildImageError::OutsideCheckout {
            field,
            path: resolved_path,
        });
    }

    Ok(resolved_path)
}

fn canonicalize(field: &'static str, path: &Path) -> Result<PathBuf, BuildImageError> {
    // Canonicalization produces the real absolute path and rejects missing inputs before
    // Podman starts, keeping filesystem failures at the build boundary.
    fs::canonicalize(path).map_err(|source| BuildImageError::ResolvePath {
        field,
        path: path.to_path_buf(),
        source,
    })
}

fn image_reference(application_name: &str, commit_sha: &str) -> String {
    format!("localhost/pneuma/{application_name}:{commit_sha}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_reference_is_determined_by_application_and_commit() {
        let commit_sha = "e".repeat(40);

        assert_eq!(
            image_reference("personal-site", &commit_sha),
            format!("localhost/pneuma/personal-site:{commit_sha}")
        );
    }
}
