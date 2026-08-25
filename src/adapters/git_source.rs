use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use thiserror::Error;

use crate::domain::git::CommitSha;

#[derive(Debug, Error)]
pub enum ResolveCommitError {
    #[error("failed to execute Git: {source}")]
    Execute {
        #[source]
        source: io::Error,
    },
    #[error(
        "failed to resolve Git revision `{revision}` in {}: {message}",
        repository_path.display()
    )]
    Resolve {
        repository_path: PathBuf,
        revision: String,
        message: String,
    },
}

#[derive(Debug, Error)]
pub enum CreateCheckoutError {
    #[error("checkout destination already exists: {}", path.display())]
    DestinationExists { path: PathBuf },
    #[error("failed to execute Git while {operation}: {source}")]
    Execute {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error(
        "Git failed while {operation} at {}: {message}",
        path.display()
    )]
    Git {
        operation: &'static str,
        path: PathBuf,
        message: String,
    },
}

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

// Resolves a local revision to a full commit while rejecting non-commit Git objects.
pub fn resolve_commit(
    repository_path: &Path,
    revision: &str,
) -> Result<CommitSha, ResolveCommitError> {
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
        .and_then(|sha| CommitSha::new(sha).ok())
        .ok_or_else(|| ResolveCommitError::Resolve {
            repository_path: repository_path.to_path_buf(),
            revision: revision.to_owned(),
            message: "Git returned an invalid commit identifier".to_owned(),
        })?;

    Ok(commit_sha)
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

// Creates an isolated detached checkout so manifest reads cannot observe mutable branch state.
pub fn create_checkout(
    repository_path: &Path,
    commit_sha: &CommitSha,
    checkout_path: &Path,
) -> Result<(), CreateCheckoutError> {
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
        .arg(commit_sha.as_str())
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

// Reuses only a clean checkout at the requested commit; stale deployment leftovers are recreated.
pub fn ensure_checkout(
    repository_path: &Path,
    commit_sha: &CommitSha,
    checkout_path: &Path,
) -> Result<(), CreateCheckoutError> {
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

// Removes a temporary checkout when present while allowing idempotent cleanup after earlier failures.
pub fn cleanup_checkout(path: &Path) -> Result<(), io::Error> {
    if path.try_exists().map_err(io::Error::other)? {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

// Confirms both the detached commit and working-tree cleanliness before a checkout may be reused.
fn is_clean_checkout_at(
    checkout_path: &Path,
    commit_sha: &CommitSha,
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
    if !head.status.success() || String::from_utf8_lossy(&head.stdout).trim() != commit_sha.as_str()
    {
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

// Selects Git's stderr diagnostic, preserving a useful exit status when it emits none.
fn git_failure_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("Git exited with {}", output.status)
    } else {
        stderr
    }
}
