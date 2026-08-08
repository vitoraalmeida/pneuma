use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::oci_image::{PullImageError, pull_image};
use crate::use_cases::deployment_create::DeploymentType;
use crate::use_cases::deployment_deploy_release::{
    DeployReleaseError, DeployedRelease, PublicDeploymentConfiguration, deploy_release,
};
use crate::use_cases::release_create::{CreateReleaseError, create_release};

#[derive(Debug)]
pub enum DeployOciError {
    PullImage { source: PullImageError },
    CreateRelease { source: CreateReleaseError },
    DeployRelease { source: DeployReleaseError },
}

impl fmt::Display for DeployOciError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
        }
    }
}

pub fn deploy_oci(
    connection: &mut Connection,
    application_id: &str,
    image_reference: &str,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeployedRelease, DeployOciError> {
    let image =
        pull_image(image_reference).map_err(|source| DeployOciError::PullImage { source })?;
    let release = create_release(
        connection,
        application_id,
        image.reference.as_str(),
        image.reference.repository(),
        image.reference.digest(),
        None,
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
