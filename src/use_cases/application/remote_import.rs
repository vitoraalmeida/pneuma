use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use thiserror::Error;

use super::import::{ImportError, import_application};
use crate::adapters::git_source::{CloneRepositoryError, cleanup_checkout, clone_repository};
use crate::domain::application::ApplicationSummary;
use crate::domain::git::is_remote_git_location;
use crate::domain::system::{InvalidSystemName, SystemName};

#[derive(Debug, Error)]
pub enum RemoteImportError {
    #[error("application imports require a Git URL; local paths are not supported")]
    InvalidRepository,
    #[error(transparent)]
    InvalidSystemName { source: InvalidSystemName },
    #[error("failed to prepare the import workspace: {source}")]
    Workspace {
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Clone { source: CloneRepositoryError },
    #[error(transparent)]
    Import { source: ImportError },
}

// Clones a remote source before opening the import transaction, then always attempts cleanup.
pub fn import_remote_application(
    connection: &mut Connection,
    repository: &str,
    workspace: &Path,
    system_name: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<ApplicationSummary, RemoteImportError> {
    if !is_remote_git_location(repository) {
        return Err(RemoteImportError::InvalidRepository);
    }
    let system_name = system_name
        .map(SystemName::new)
        .transpose()
        .map_err(|source| RemoteImportError::InvalidSystemName { source })?;
    let temporary_root = workspace.join("imports");
    fs::create_dir_all(&temporary_root)
        .map_err(|source| RemoteImportError::Workspace { source })?;
    let checkout = temporary_root.join(unique_suffix());
    if let Err(source) = clone_repository(repository, &checkout) {
        let _ = cleanup_checkout(&checkout);
        return Err(RemoteImportError::Clone { source });
    }

    let result = import_application(
        connection,
        &checkout,
        system_name.as_ref(),
        repository,
        manifest_path,
    )
    .map_err(|source| RemoteImportError::Import { source });
    let _ = cleanup_checkout(&checkout);
    result
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}
