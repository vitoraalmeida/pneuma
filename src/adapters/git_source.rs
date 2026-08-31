use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use thiserror::Error;

use crate::domain::git::CommitSha;

#[derive(Debug, Error)]
pub enum CloneRepositoryError {
    #[error("checkout destination already exists: {}", path.display())]
    DestinationExists { path: PathBuf },
    #[error("failed to execute Git while {operation}: {source}")]
    Execute {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("Git failed while {operation} of `{url}`: {message}")]
    Git {
        operation: &'static str,
        url: String,
        message: String,
    },
}

#[derive(Debug, Error)]
pub enum ResolveBranchError {
    #[error("failed to execute Git: {source}")]
    Execute {
        #[source]
        source: io::Error,
    },
    #[error("Git repository `{url}` was not found or is unreachable")]
    RepositoryNotFound { url: String },
    #[error("authentication failed for Git repository `{url}`")]
    AuthenticationFailed { url: String },
    #[error("branch or tag `{branch}` was not found in Git repository `{url}`")]
    BranchNotFound { url: String, branch: String },
    #[error("Git returned an invalid commit for branch or tag `{branch}` in `{url}`: {message}")]
    InvalidCommit {
        url: String,
        branch: String,
        message: String,
    },
}

// Resolves a remote branch or tag in precedence order without cloning the repository.
pub fn resolve_branch(url: &str, branch: &str) -> Result<CommitSha, ResolveBranchError> {
    for pattern in [
        format!("refs/heads/{branch}"),
        format!("refs/tags/{branch}^{{}}"),
        format!("refs/tags/{branch}"),
    ] {
        let output = Command::new("git")
            .args(["ls-remote", "--", url, &pattern])
            .output()
            .map_err(|source| ResolveBranchError::Execute { source })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Authentication failed")
                || stderr.contains("Permission denied")
                || stderr.contains("could not read Username")
            {
                return Err(ResolveBranchError::AuthenticationFailed {
                    url: url.to_owned(),
                });
            }
            return Err(ResolveBranchError::RepositoryNotFound {
                url: url.to_owned(),
            });
        }

        if let Some(sha) = parse_ls_remote_sha(&output.stdout) {
            return CommitSha::new(sha).map_err(|message| ResolveBranchError::InvalidCommit {
                url: url.to_owned(),
                branch: branch.to_owned(),
                message: message.to_string(),
            });
        }
    }

    Err(ResolveBranchError::BranchNotFound {
        url: url.to_owned(),
        branch: branch.to_owned(),
    })
}

// Extracts the first complete commit identifier from Git's tab-separated remote listing.
fn parse_ls_remote_sha(stdout: &[u8]) -> Option<&str> {
    std::str::from_utf8(stdout)
        .ok()?
        .lines()
        .find_map(|line| line.split('\t').next())
        .filter(|sha| CommitSha::new(sha).is_ok())
}

// Clones a remote repository into a create-only destination to avoid replacing existing workspace state.
pub fn clone_repository(url: &str, destination: &Path) -> Result<(), CloneRepositoryError> {
    if destination
        .try_exists()
        .map_err(|source| CloneRepositoryError::Execute {
            operation: "inspecting the checkout destination",
            source,
        })?
    {
        return Err(CloneRepositoryError::DestinationExists {
            path: destination.to_path_buf(),
        });
    }

    let clone = Command::new("git")
        .args(["clone", "--quiet", "--"])
        .arg(url)
        .arg(destination)
        .output()
        .map_err(|source| CloneRepositoryError::Execute {
            operation: "cloning the remote repository",
            source,
        })?;
    if !clone.status.success() {
        return Err(CloneRepositoryError::Git {
            operation: "cloning the remote repository",
            url: url.to_owned(),
            message: git_failure_message(&clone),
        });
    }

    Ok(())
}

// Selects Git's stderr diagnostic, preserving a useful exit status when it emits none.
fn git_failure_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("Git exited with {}", output.status)
    } else {
        stderr
    }
}

// Removes a temporary checkout when present while allowing idempotent cleanup after earlier failures.
pub fn cleanup_checkout(path: &Path) -> Result<(), io::Error> {
    if path.try_exists().map_err(io::Error::other)? {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}
