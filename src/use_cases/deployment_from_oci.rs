use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::oci_image::{PullImageError, pull_image};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::deployment::{DeploymentType, SourceRevision};
use crate::domain::git::CommitSha;
use crate::domain::identity::ApplicationId;
use crate::domain::release::OciArtifact;
use crate::use_cases::deployment_execute_release::{
    DeployReleaseError, DeploymentResult, PublicDeploymentConfiguration, deploy_release,
    deploy_release_with_progress,
};
use crate::use_cases::deployment_progress::DeploymentProgress;
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
        source: ApplicationStoreError,
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

// Validates the requested artifact against delivery policy, pulls it, records a release, and
// delegates runtime orchestration to the release deployment workflow.
pub fn deploy_oci(
    connection: &mut Connection,
    application_id: &ApplicationId,
    image_reference: &str,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, DeployOciError> {
    deploy_oci_reporting(
        connection,
        application_id,
        image_reference,
        source_commit,
        public_configuration,
        None,
    )
}

// Deploys an OCI artifact while forwarding lifecycle progress to the caller.
pub fn deploy_oci_with_progress(
    connection: &mut Connection,
    application_id: &ApplicationId,
    image_reference: &str,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut dyn FnMut(DeploymentProgress),
) -> Result<DeploymentResult, DeployOciError> {
    deploy_oci_reporting(
        connection,
        application_id,
        image_reference,
        source_commit,
        public_configuration,
        Some(progress),
    )
}

fn deploy_oci_reporting(
    connection: &mut Connection,
    application_id: &ApplicationId,
    image_reference: &str,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: Option<&mut dyn FnMut(DeploymentProgress)>,
) -> Result<DeploymentResult, DeployOciError> {
    let artifact =
        OciArtifact::parse(image_reference).map_err(|source| DeployOciError::PullImage {
            source: PullImageError::InvalidReference { source },
        })?;
    let delivery =
        application_store::load_delivery_specification(connection, application_id.as_str())
            .map_err(|source| DeployOciError::DeliveryConfiguration { source })?;
    let Some(delivery) = delivery else {
        return Err(DeployOciError::NoDeliveryConfiguration {
            application_id: application_id.to_string(),
        });
    };
    if artifact.repository() != delivery.image_repository().as_str() {
        return Err(DeployOciError::RepositoryMismatch {
            application_id: application_id.to_string(),
            allowed: delivery.image_repository().as_str().to_owned(),
            actual: artifact.repository().to_owned(),
        });
    }
    let image =
        pull_image(artifact.reference()).map_err(|source| DeployOciError::PullImage { source })?;
    let release = create_release(connection, application_id, &image.artifact)
        .map_err(|source| DeployOciError::CreateRelease { source })?;
    let source_revision = source_commit.cloned().map(SourceRevision::from_commit);
    let deployed = match progress {
        Some(progress) => deploy_release_with_progress(
            connection,
            application_id,
            &release,
            DeploymentType::Deploy,
            source_revision.as_ref(),
            public_configuration,
            progress,
        ),
        None => deploy_release(
            connection,
            application_id,
            &release,
            DeploymentType::Deploy,
            source_revision.as_ref(),
            public_configuration,
        ),
    };
    deployed.map_err(|source| DeployOciError::DeployRelease { source })
}
