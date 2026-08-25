use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use super::execute::{
    DeployReleaseError, DeploymentResult, PublicDeploymentConfiguration, deploy_release_reporting,
};
use super::progress::{DeploymentProgress, ProgressReporter};
use crate::adapters::git_source::{ResolveBranchError, resolve_branch};
use crate::adapters::oci_image::{
    PullImageError, ResolveImageDigestError, pull_image, resolve_image_digest,
};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::deployment::{DeploymentType, SourceRevision};
use crate::domain::git::CommitSha;
use crate::domain::identity::ApplicationId;
use crate::domain::release::{DeliverySpecification, OciArtifact};
use crate::use_cases::release::{CreateReleaseError, create_release};

#[derive(Debug)]
pub enum DeployBranchError {
    NoSourceConfiguration { application_id: String },
    NoDefaultBranch { application_id: String },
    NoDeliveryConfiguration { application_id: String },
    SourceConfiguration { source: ApplicationStoreError },
    ResolveBranch { source: ResolveBranchError },
    ResolveImageDigest { source: ResolveImageDigestError },
    DeployOci { source: DeployOciError },
}

impl fmt::Display for DeployBranchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSourceConfiguration { application_id } => write!(
                formatter,
                "application `{application_id}` has no source configuration; re-import its manifest from a Git repository"
            ),
            Self::NoDefaultBranch { application_id } => write!(
                formatter,
                "application `{application_id}` has no default branch and no branch was specified"
            ),
            Self::NoDeliveryConfiguration { application_id } => write!(
                formatter,
                "application `{application_id}` has no delivery configuration; re-import its manifest with a [delivery] section"
            ),
            Self::SourceConfiguration { source } => {
                write!(formatter, "failed to load source configuration: {source}")
            }
            Self::ResolveBranch { source } => {
                write!(formatter, "failed to resolve branch: {source}")
            }
            Self::ResolveImageDigest { source } => {
                write!(formatter, "failed to resolve image digest: {source}")
            }
            Self::DeployOci { source } => write!(formatter, "{source}"),
        }
    }
}

impl Error for DeployBranchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceConfiguration { source } => Some(source),
            Self::ResolveBranch { source } => Some(source),
            Self::ResolveImageDigest { source } => Some(source),
            Self::DeployOci { source } => Some(source),
            Self::NoSourceConfiguration { .. }
            | Self::NoDefaultBranch { .. }
            | Self::NoDeliveryConfiguration { .. } => None,
        }
    }
}

// Resolves a branch to its immutable commit and image digest before delegating to OCI deployment.
pub fn deploy_branch(
    connection: &mut Connection,
    application_id: &ApplicationId,
    branch: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, DeployBranchError> {
    let mut progress = ProgressReporter::disabled();
    deploy_branch_reporting(
        connection,
        application_id,
        branch,
        public_configuration,
        &mut progress,
    )
}

// Resolves and deploys a branch while forwarding deployment lifecycle progress to the caller.
pub fn deploy_branch_with_progress(
    connection: &mut Connection,
    application_id: &ApplicationId,
    branch: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut dyn FnMut(DeploymentProgress),
) -> Result<DeploymentResult, DeployBranchError> {
    let mut progress = ProgressReporter::enabled(progress);
    deploy_branch_reporting(
        connection,
        application_id,
        branch,
        public_configuration,
        &mut progress,
    )
}

fn deploy_branch_reporting(
    connection: &mut Connection,
    application_id: &ApplicationId,
    branch: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeploymentResult, DeployBranchError> {
    let source = application_store::load_source(connection, application_id)
        .map_err(|source| DeployBranchError::SourceConfiguration { source })?
        .ok_or_else(|| DeployBranchError::NoSourceConfiguration {
            application_id: application_id.to_string(),
        })?;

    let branch = match branch {
        Some(branch) => branch.to_owned(),
        None => source.default_branch().map(str::to_owned).ok_or_else(|| {
            DeployBranchError::NoDefaultBranch {
                application_id: application_id.to_string(),
            }
        })?,
    };

    let commit_sha: CommitSha = resolve_branch(source.repository_location(), &branch)
        .map_err(|source| DeployBranchError::ResolveBranch { source })?;

    let delivery = application_store::load_delivery_specification(connection, application_id)
        .map_err(|source| DeployBranchError::SourceConfiguration { source })?
        .ok_or_else(|| DeployBranchError::NoDeliveryConfiguration {
            application_id: application_id.to_string(),
        })?;

    let reference = resolve_image_digest(delivery.image_repository(), &commit_sha)
        .map_err(|source| DeployBranchError::ResolveImageDigest { source })?;

    // The delivery policy was already loaded above; pass it down instead of re-reading it.
    deploy_artifact_for_delivery(
        connection,
        application_id,
        &reference,
        Some(&commit_sha),
        public_configuration,
        &delivery,
        progress,
    )
    .map_err(|source| DeployBranchError::DeployOci { source })
}

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

// Validates the requested artifact against the persisted delivery policy, pulls it, records a
// release, and delegates runtime orchestration to the release deployment workflow.
pub fn deploy_oci(
    connection: &mut Connection,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, DeployOciError> {
    let mut progress = ProgressReporter::disabled();
    deploy_oci_reporting(
        connection,
        application_id,
        artifact,
        source_commit,
        public_configuration,
        &mut progress,
    )
}

// Deploys an OCI artifact while forwarding lifecycle progress to the caller.
pub fn deploy_oci_with_progress(
    connection: &mut Connection,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut dyn FnMut(DeploymentProgress),
) -> Result<DeploymentResult, DeployOciError> {
    let mut progress = ProgressReporter::enabled(progress);
    deploy_oci_reporting(
        connection,
        application_id,
        artifact,
        source_commit,
        public_configuration,
        &mut progress,
    )
}

fn deploy_oci_reporting(
    connection: &mut Connection,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeploymentResult, DeployOciError> {
    let delivery = application_store::load_delivery_specification(connection, application_id)
        .map_err(|source| DeployOciError::DeliveryConfiguration { source })?;
    let Some(delivery) = delivery else {
        return Err(DeployOciError::NoDeliveryConfiguration {
            application_id: application_id.to_string(),
        });
    };
    deploy_artifact_for_delivery(
        connection,
        application_id,
        artifact,
        source_commit,
        public_configuration,
        &delivery,
        progress,
    )
}

// Checks an artifact against caller-established delivery policy, pulls it, records a release,
// and delegates runtime orchestration to the release deployment workflow. Callers that already
// loaded the specification pass it down instead of re-reading it from the store.
fn deploy_artifact_for_delivery(
    connection: &mut Connection,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    delivery: &DeliverySpecification,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeploymentResult, DeployOciError> {
    if !delivery.permits(artifact) {
        return Err(DeployOciError::RepositoryMismatch {
            application_id: application_id.to_string(),
            allowed: delivery.image_repository().as_str().to_owned(),
            actual: artifact.repository().to_owned(),
        });
    }
    let image = pull_image(artifact).map_err(|source| DeployOciError::PullImage { source })?;
    let release = create_release(connection, application_id, &image.artifact)
        .map_err(|source| DeployOciError::CreateRelease { source })?;
    let source_revision = source_commit.cloned().map(SourceRevision::from_commit);
    deploy_release_reporting(
        connection,
        application_id,
        &release,
        DeploymentType::Deploy,
        source_revision.as_ref(),
        public_configuration,
        progress,
    )
    .map_err(|source| DeployOciError::DeployRelease { source })
}
