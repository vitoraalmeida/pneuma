use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::adapters::git_source::{
    CloneRepositoryError, cleanup_checkout, clone_repository, is_remote_repository,
};
use crate::domain::application::ApplicationSummary;
use crate::domain::system::{InvalidSystemName, SystemName};
use crate::use_cases::application_import::{ImportError, import_application};

#[derive(Debug)]
pub enum RemoteImportError {
    InvalidRepository,
    InvalidSystemName { source: InvalidSystemName },
    Workspace { source: std::io::Error },
    Clone { source: CloneRepositoryError },
    Import { source: ImportError },
}

impl fmt::Display for RemoteImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRepository => formatter
                .write_str("application imports require a Git URL; local paths are not supported"),
            Self::InvalidSystemName { source } => write!(formatter, "{source}"),
            Self::Clone { source } => write!(formatter, "{source}"),
            Self::Import { source } => write!(formatter, "{source}"),
            Self::Workspace { source } => {
                write!(
                    formatter,
                    "failed to prepare the import workspace: {source}"
                )
            }
        }
    }
}

impl Error for RemoteImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidSystemName { source } => Some(source),
            Self::Workspace { source } => Some(source),
            Self::Clone { source } => Some(source),
            Self::Import { source } => Some(source),
            Self::InvalidRepository => None,
        }
    }
}

// Clones a remote source before opening the import transaction, then always attempts cleanup.
pub fn import_remote_application(
    connection: &mut Connection,
    repository: &str,
    workspace: &Path,
    system_name: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<ApplicationSummary, RemoteImportError> {
    if !is_remote_repository(repository) {
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
        Some(repository),
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
