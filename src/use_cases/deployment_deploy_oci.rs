use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension};

use crate::adapters::oci_image::{OciImageReference, PullImageError, pull_image};
use crate::domain::deployment::DeploymentType;
use crate::use_cases::deployment_deploy_release::{
    DeployReleaseError, DeployedRelease, PublicDeploymentConfiguration, deploy_release,
};
use crate::use_cases::release_create::{CreateReleaseError, create_release};

#[derive(Debug)]
pub enum DeployOciError {
    NoDeliveryConfiguration {
        application_id: String,
    },
    RepositoryMismatch {
        application_id: String,
        allowed: String,
        actual: String,
    },
    DeliveryConfiguration {
        source: rusqlite::Error,
    },
    PullImage {
        source: PullImageError,
    },
    CreateRelease {
        source: CreateReleaseError,
    },
    DeployRelease {
        source: DeployReleaseError,
    },
}

impl fmt::Display for DeployOciError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDeliveryConfiguration { application_id } => write!(
                formatter,
                "application `{application_id}` has no delivery configuration; re-import its manifest with a [delivery] section"
            ),
            Self::RepositoryMismatch {
                application_id,
                allowed,
                actual,
            } => write!(
                formatter,
                "application `{application_id}` only accepts images from `{allowed}`, not `{actual}`"
            ),
            Self::DeliveryConfiguration { source } => {
                write!(formatter, "failed to load delivery configuration: {source}")
            }
            Self::PullImage { source } => write!(formatter, "failed to pull OCI image: {source}"),
            Self::CreateRelease { source } => write!(formatter, "{source}"),
            Self::DeployRelease { source } => write!(formatter, "{source}"),
        }
    }
}

impl Error for DeployOciError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::PullImage { source } => Some(source),
            Self::CreateRelease { source } => Some(source),
            Self::DeployRelease { source } => Some(source),
            Self::DeliveryConfiguration { source } => Some(source),
            Self::NoDeliveryConfiguration { .. } | Self::RepositoryMismatch { .. } => None,
        }
    }
}

pub fn deploy_oci(
    connection: &mut Connection,
    application_id: &str,
    image_reference: &str,
    source_revision: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeployedRelease, DeployOciError> {
    let reference =
        OciImageReference::parse(image_reference).map_err(|source| DeployOciError::PullImage {
            source: PullImageError::InvalidReference { source },
        })?;
    let allowed_repository: Option<String> = connection
        .query_row(
            "SELECT image_repository
             FROM application_delivery_specs
             WHERE application_id = ?1",
            [application_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| DeployOciError::DeliveryConfiguration { source })?;
    let Some(allowed_repository) = allowed_repository else {
        return Err(DeployOciError::NoDeliveryConfiguration {
            application_id: application_id.to_owned(),
        });
    };
    if reference.repository() != allowed_repository {
        return Err(DeployOciError::RepositoryMismatch {
            application_id: application_id.to_owned(),
            allowed: allowed_repository,
            actual: reference.repository().to_owned(),
        });
    }
    let image =
        pull_image(image_reference).map_err(|source| DeployOciError::PullImage { source })?;
    let release = create_release(
        connection,
        application_id,
        image.reference.as_str(),
        image.reference.repository(),
        image.reference.digest(),
        source_revision,
    )
    .map_err(|source| DeployOciError::CreateRelease { source })?;
    deploy_release(
        connection,
        application_id,
        &release,
        DeploymentType::Deploy,
        public_configuration,
    )
    .map_err(|source| DeployOciError::DeployRelease { source })
}
