use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Debug)]
pub enum ResolveCommitError {
    Execute {
        source: io::Error,
    },
    Resolve {
        repository_path: PathBuf,
        revision: String,
        message: String,
    },
}

#[derive(Debug)]
pub enum CreateCheckoutError {
    InvalidCommit,
    DestinationExists {
        path: PathBuf,
    },
    Execute {
        operation: &'static str,
        source: io::Error,
    },
    Git {
        operation: &'static str,
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for ResolveCommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Execute { source } => write!(formatter, "failed to execute Git: {source}"),
            Self::Resolve {
                repository_path,
                revision,
                message,
            } => write!(
                formatter,
                "failed to resolve Git revision `{revision}` in {}: {message}",
                repository_path.display()
            ),
        }
    }
}

impl Error for ResolveCommitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source } => Some(source),
            Self::Resolve { .. } => None,
        }
    }
}

impl fmt::Display for CreateCheckoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCommit => formatter.write_str("commit identifier must be hexadecimal"),
            Self::DestinationExists { path } => {
                write!(
                    formatter,
                    "checkout destination already exists: {}",
                    path.display()
                )
            }
            Self::Execute { operation, source } => {
                write!(
                    formatter,
                    "failed to execute Git while {operation}: {source}"
                )
            }
            Self::Git {
                operation,
                path,
                message,
            } => write!(
                formatter,
                "Git failed while {operation} at {}: {message}",
                path.display()
            ),
        }
    }
}

impl Error for CreateCheckoutError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source, .. } => Some(source),
            Self::InvalidCommit | Self::DestinationExists { .. } | Self::Git { .. } => None,
        }
    }
}

pub fn resolve_commit(
    repository_path: &Path,
    revision: &str,
) -> Result<String, ResolveCommitError> {
    // Branches and tags can move, so peel the requested revision to an immutable commit.
    // The commit suffix also rejects blobs and trees before downstream operations use it.
    let revision_expression = format!("{revision}^{{commit}}");
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args(["rev-parse", "--verify", "--end-of-options"])
        .arg(revision_expression)
        .output()
        .map_err(|source| ResolveCommitError::Execute { source })?;

    if !output.status.success() {
        let message = git_failure_message(&output);
        return Err(ResolveCommitError::Resolve {
            repository_path: repository_path.to_path_buf(),
            revision: revision.to_owned(),
            message,
        });
    }

    let commit_sha = std::str::from_utf8(&output.stdout)
        .map(str::trim)
        .ok()
        .filter(|sha| !sha.is_empty() && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| ResolveCommitError::Resolve {
            repository_path: repository_path.to_path_buf(),
            revision: revision.to_owned(),
            message: "Git returned an invalid commit identifier".to_owned(),
        })?;

    Ok(commit_sha.to_owned())
}

pub fn create_checkout(
    repository_path: &Path,
    commit_sha: &str,
    checkout_path: &Path,
) -> Result<(), CreateCheckoutError> {
    if commit_sha.is_empty() || !commit_sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CreateCheckoutError::InvalidCommit);
    }

    if checkout_path
        .try_exists()
        .map_err(|source| CreateCheckoutError::Execute {
            operation: "inspecting the checkout destination",
            source,
        })?
    {
        return Err(CreateCheckoutError::DestinationExists {
            path: checkout_path.to_path_buf(),
        });
    }

    let clone = Command::new("git")
        // Temporary checkouts can live on another filesystem, where Git cannot use
        // the hard links normally attempted by a local clone. Copy objects instead.
        .args([
            "clone",
            "--quiet",
            "--no-checkout",
            "--local",
            "--no-hardlinks",
            "--",
        ])
        .arg(repository_path)
        .arg(checkout_path)
        .output()
        .map_err(|source| CreateCheckoutError::Execute {
            operation: "creating an isolated checkout",
            source,
        })?;
    if !clone.status.success() {
        return Err(CreateCheckoutError::Git {
            operation: "creating an isolated checkout",
            path: checkout_path.to_path_buf(),
            message: git_failure_message(&clone),
        });
    }

    let checkout = Command::new("git")
        .arg("-C")
        .arg(checkout_path)
        .args(["checkout", "--quiet", "--detach"])
        .arg(commit_sha)
        .output()
        .map_err(|source| CreateCheckoutError::Execute {
            operation: "checking out the resolved commit",
            source,
        })?;
    if !checkout.status.success() {
        let _ = fs::remove_dir_all(checkout_path);
        return Err(CreateCheckoutError::Git {
            operation: "checking out the resolved commit",
            path: checkout_path.to_path_buf(),
            message: git_failure_message(&checkout),
        });
    }

    Ok(())
}

pub fn ensure_checkout(
    repository_path: &Path,
    commit_sha: &str,
    checkout_path: &Path,
) -> Result<(), CreateCheckoutError> {
    if commit_sha.is_empty() || !commit_sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CreateCheckoutError::InvalidCommit);
    }

    let exists = checkout_path
        .try_exists()
        .map_err(|source| CreateCheckoutError::Execute {
            operation: "inspecting the checkout destination",
            source,
        })?;
    if !exists {
        return create_checkout(repository_path, commit_sha, checkout_path);
    }

    if is_clean_checkout_at(checkout_path, commit_sha)? {
        return Ok(());
    }

    // A failed deployment can leave a checkout behind; discard an unusable or
    // stale one so a retry of the same commit starts from a clean tree.
    fs::remove_dir_all(checkout_path).map_err(|source| CreateCheckoutError::Execute {
        operation: "removing an unusable checkout",
        source,
    })?;
    create_checkout(repository_path, commit_sha, checkout_path)
}

fn is_clean_checkout_at(
    checkout_path: &Path,
    commit_sha: &str,
) -> Result<bool, CreateCheckoutError> {
    let head = Command::new("git")
        .arg("-C")
        .arg(checkout_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|source| CreateCheckoutError::Execute {
            operation: "inspecting an existing checkout",
            source,
        })?;
    if !head.status.success() || String::from_utf8_lossy(&head.stdout).trim() != commit_sha {
        return Ok(false);
    }

    let status = Command::new("git")
        .arg("-C")
        .arg(checkout_path)
        .args(["status", "--porcelain"])
        .output()
        .map_err(|source| CreateCheckoutError::Execute {
            operation: "inspecting an existing checkout",
            source,
        })?;
    Ok(status.status.success() && String::from_utf8_lossy(&status.stdout).trim().is_empty())
}

fn git_failure_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("Git exited with {}", output.status)
    } else {
        stderr
    }
}
