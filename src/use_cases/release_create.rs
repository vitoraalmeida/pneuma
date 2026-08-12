use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::release_store::{self, ReleaseStoreError};
use crate::domain::release::Release;

#[derive(Debug)]
pub enum CreateReleaseError {
    ApplicationNotFound { application_id: String },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for CreateReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
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
            ApplicationStoreError::SystemNotFound { .. } => Self::ApplicationNotFound {
                application_id: "unknown".to_owned(),
            },
            ApplicationStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

impl From<ReleaseStoreError> for CreateReleaseError {
    fn from(error: ReleaseStoreError) -> Self {
        match error {
            ReleaseStoreError::NotFound { .. } => Self::ApplicationNotFound {
                application_id: "unknown".to_owned(),
            },
            ReleaseStoreError::ApplicationNotFound { application_id } => {
                Self::ApplicationNotFound { application_id }
            }
            ReleaseStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

pub fn create_release(
    connection: &mut Connection,
    application_id: &str,
    image_reference: &str,
    image_repository: &str,
    image_digest: &str,
) -> Result<Release, CreateReleaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    let exists = application_store::application_exists(&transaction, application_id)?;
    if !exists {
        return Err(CreateReleaseError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        });
    }

    let release_id = release_store::generate_id(&transaction)?;

    release_store::insert_release(
        &transaction,
        &release_id,
        application_id,
        image_reference,
        image_repository,
        image_digest,
    )?;

    let release =
        release_store::load_release_by_digest(&transaction, application_id, image_digest)?;

    transaction
        .commit()
        .map_err(|source| CreateReleaseError::Persistence { source })?;

    Ok(release)
}
