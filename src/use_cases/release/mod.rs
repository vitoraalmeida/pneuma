use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::release_store::{self, ReleaseStoreError};
use crate::domain::identity::ApplicationId;
use crate::domain::release::{OciArtifact, Release};

#[derive(Debug, Error)]
pub enum CreateReleaseError {
    #[error("application `{application_id}` was not found")]
    ApplicationNotFound { application_id: String },
    #[error("failed to create release: {source}")]
    ApplicationStore {
        #[source]
        source: ApplicationStoreError,
    },
    #[error("failed to create release: {source}")]
    ReleaseStore {
        #[source]
        source: ReleaseStoreError,
    },
    #[error("failed to create release: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

impl From<ApplicationStoreError> for CreateReleaseError {
    fn from(error: ApplicationStoreError) -> Self {
        match error {
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

    let exists = application_store::application_exists(&transaction, application_id)?;
    if !exists {
        return Err(CreateReleaseError::ApplicationNotFound {
            application_id: application_id.to_string(),
        });
    }

    let release_id = release_store::generate_id(&transaction)?;

    release_store::insert_release(&transaction, &release_id, application_id, artifact)?;

    let release =
        release_store::load_release_by_digest(&transaction, application_id, artifact.digest())?;

    transaction
        .commit()
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    Ok(release)
}
