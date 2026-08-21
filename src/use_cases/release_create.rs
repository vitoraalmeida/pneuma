use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::release_store::{self, ReleaseStoreError};
use crate::domain::identity::ApplicationId;
use crate::domain::release::{OciArtifact, Release};

#[derive(Debug)]
pub enum CreateReleaseError {
    ApplicationNotFound { application_id: String },
    ApplicationStore { source: ApplicationStoreError },
    ReleaseStore { source: ReleaseStoreError },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for CreateReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to create release: {source}")
            }
            Self::ReleaseStore { source } => {
                write!(formatter, "failed to create release: {source}")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to create release: {source}")
            }
        }
    }
}

impl Error for CreateReleaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ApplicationNotFound { .. } => None,
            Self::ApplicationStore { source } => Some(source),
            Self::ReleaseStore { source } => Some(source),
            Self::Persistence { source } => Some(source),
        }
    }
}

impl From<ApplicationStoreError> for CreateReleaseError {
    fn from(error: ApplicationStoreError) -> Self {
        match error {
            ApplicationStoreError::NotFound { application_id } => {
                Self::ApplicationNotFound { application_id }
            }
            ApplicationStoreError::InvalidDesiredRuntimeState { .. } => {
                Self::ApplicationStore { source: error }
            }
            ApplicationStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

impl From<ReleaseStoreError> for CreateReleaseError {
    fn from(error: ReleaseStoreError) -> Self {
        match error {
            ReleaseStoreError::NotFound { .. } | ReleaseStoreError::NotFoundByArtifact { .. } => {
                Self::ReleaseStore { source: error }
            }
            ReleaseStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

// Creates or resolves a digest-pinned release within one short persistence transaction.
pub fn create_release(
    connection: &mut Connection,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
) -> Result<Release, CreateReleaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    let exists = application_store::application_exists(&transaction, application_id.as_str())?;
    if !exists {
        return Err(CreateReleaseError::ApplicationNotFound {
            application_id: application_id.to_string(),
        });
    }

    let release_id = release_store::generate_id(&transaction)?;

    release_store::insert_release(&transaction, &release_id, application_id.as_str(), artifact)?;

    let release = release_store::load_release_by_digest(
        &transaction,
        application_id.as_str(),
        artifact.digest(),
    )?;

    transaction
        .commit()
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    Ok(release)
}
