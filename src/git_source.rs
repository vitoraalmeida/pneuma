use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
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
