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

#[derive(Debug)]
pub enum CloneRepositoryError {
    DestinationExists {
        path: PathBuf,
    },
    Execute {
        operation: &'static str,
        source: io::Error,
    },
    Git {
        operation: &'static str,
        url: String,
        message: String,
    },
}

#[derive(Debug)]
pub enum ResolveBranchError {
    Execute {
        source: io::Error,
    },
    RepositoryNotFound {
        url: String,
    },
    AuthenticationFailed {
        url: String,
    },
    BranchNotFound {
        url: String,
        branch: String,
    },
    InvalidCommit {
        url: String,
        branch: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents a validated full Git object ID so downstream delivery always uses immutable commits.
pub struct CommitSha(String);

impl CommitSha {
    // Validates the full SHA-1 commit identifier returned or supplied at the Git boundary.
    pub fn new(sha: &str) -> Result<Self, String> {
        if !is_commit_sha(sha) {
            return Err("commit identifier must be exactly 40 hexadecimal characters".to_owned());
        }
        Ok(Self(sha.to_owned()))
    }

    // Exposes the validated identifier for Git and OCI tag construction.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
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

impl fmt::Display for CloneRepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
                url,
                message,
            } => write!(
                formatter,
                "Git failed while {operation} of `{url}`: {message}"
            ),
        }
    }
}

impl Error for CloneRepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source, .. } => Some(source),
            Self::DestinationExists { .. } | Self::Git { .. } => None,
        }
    }
}

impl fmt::Display for ResolveBranchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Execute { source } => write!(formatter, "failed to execute Git: {source}"),
            Self::RepositoryNotFound { url } => {
                write!(
                    formatter,
                    "Git repository `{url}` was not found or is unreachable"
                )
            }
            Self::AuthenticationFailed { url } => write!(
                formatter,
                "authentication failed for Git repository `{url}`"
            ),
            Self::BranchNotFound { url, branch } => {
                write!(
                    formatter,
                    "branch or tag `{branch}` was not found in Git repository `{url}`"
                )
            }
            Self::InvalidCommit {
                url,
                branch,
                message,
            } => write!(
                formatter,
                "Git returned an invalid commit for branch or tag `{branch}` in `{url}`: {message}"
            ),
        }
    }
}

impl Error for ResolveBranchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source } => Some(source),
            Self::RepositoryNotFound { .. }
            | Self::AuthenticationFailed { .. }
            | Self::BranchNotFound { .. }
            | Self::InvalidCommit { .. } => None,
        }
    }
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
                message,
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
        .filter(|sha| is_commit_sha(sha))
}

// Restricts commit identifiers to complete hexadecimal SHA-1 values rather than abbreviated revisions.
fn is_commit_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

// Creates an isolated detached checkout so manifest reads cannot observe mutable branch state.
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

// Reuses only a clean checkout at the requested commit; stale deployment leftovers are recreated.
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

// Identifies supported remote URL forms before import decides whether cloning is required.
pub fn is_remote_repository(repository: &str) -> bool {
    repository.contains("://") || repository.starts_with("git@")
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

// Selects Git's stderr diagnostic, preserving a useful exit status when it emits none.
fn git_failure_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("Git exited with {}", output.status)
    } else {
        stderr
    }
}
