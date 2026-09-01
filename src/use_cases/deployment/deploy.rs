use rusqlite::Connection;
use thiserror::Error;

use super::execute::{DeploymentResult, PublicDeploymentConfiguration, deploy_release_reporting};
use super::failure::DeployReleaseError;
use super::progress::{DeploymentEvent, DeploymentStep, EventReporter};
use crate::adapters::application_lock::{ApplicationLock, ApplicationLockError};
use crate::adapters::git_source::{ResolveBranchError, resolve_branch};
use crate::adapters::oci_image::{
    PullImageError, ResolveImageDigestError, pull_image, resolve_image_digest,
};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::deployment::DeploymentType;
use crate::domain::git::CommitSha;
use crate::domain::identity::ApplicationId;
use crate::domain::release::{DeliverySpecification, OciArtifact};
use crate::use_cases::release::{CreateReleaseError, create_release_while_locked};

#[derive(Debug, Error)]
pub enum DeployBranchError {
    #[error("failed to acquire deployment lock: {source}")]
    ApplicationLock {
        #[source]
        source: ApplicationLockError,
    },
    #[error("application `{application_id}` already has an operation in progress")]
    ApplicationBusy { application_id: String },
    #[error(
        "application `{application_id}` has no source configuration; re-import its manifest from a Git repository"
    )]
    NoSourceConfiguration { application_id: String },
    #[error("application `{application_id}` has no default branch and no branch was specified")]
    NoDefaultBranch { application_id: String },
    #[error(
        "application `{application_id}` has no delivery configuration; re-import its manifest with a [delivery] section"
    )]
    NoDeliveryConfiguration { application_id: String },
    #[error("failed to load source configuration: {source}")]
    SourceConfiguration {
        #[source]
        source: ApplicationStoreError,
    },
    #[error("failed to resolve branch: {source}")]
    ResolveBranch {
        #[source]
        source: ResolveBranchError,
    },
    #[error("failed to resolve image digest: {source}")]
    ResolveImageDigest {
        #[source]
        source: ResolveImageDigestError,
    },
    #[error(transparent)]
    DeployOci { source: DeployOciError },
}

// Resolves a branch to its immutable commit and image digest before delegating to OCI deployment.
pub fn deploy_branch(
    connection: &mut Connection,
    application_id: &ApplicationId,
    branch: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, DeployBranchError> {
    let mut events = EventReporter::disabled();
    deploy_branch_reporting(
        connection,
        application_id,
        branch,
        public_configuration,
        &mut events,
    )
}

fn deploy_branch_reporting(
    connection: &mut Connection,
    application_id: &ApplicationId,
    branch: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    events: &mut EventReporter<'_>,
) -> Result<DeploymentResult, DeployBranchError> {
    let Some(_lock) = ApplicationLock::try_acquire_for_connection(connection, application_id)
        .map_err(|source| DeployBranchError::ApplicationLock { source })?
    else {
        return Err(DeployBranchError::ApplicationBusy {
            application_id: application_id.to_string(),
        });
    };
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

    events.started(DeploymentStep::ResolveBranch);
    let commit_sha: CommitSha = resolve_branch(source.repository_url(), &branch)
        .map_err(|source| DeployBranchError::ResolveBranch { source })?;
    events.completed(DeploymentStep::ResolveBranch);

    let delivery = application_store::load_delivery_specification(connection, application_id)
        .map_err(|source| DeployBranchError::SourceConfiguration { source })?
        .ok_or_else(|| DeployBranchError::NoDeliveryConfiguration {
            application_id: application_id.to_string(),
        })?;

    events.started(DeploymentStep::ResolveImageDigest);
    let reference = resolve_image_digest(delivery.image_repository(), &commit_sha)
        .map_err(|source| DeployBranchError::ResolveImageDigest { source })?;
    events.completed(DeploymentStep::ResolveImageDigest);

    // The delivery policy was already loaded above; pass it down instead of re-reading it.
    deploy_artifact_for_delivery(
        connection,
        application_id,
        &reference,
        Some(&commit_sha),
        public_configuration,
        &delivery,
        events,
    )
    .map_err(|source| DeployBranchError::DeployOci { source })
}

// Resolves and deploys a branch while forwarding semantic workflow events to the caller.
pub fn deploy_branch_with_events(
    connection: &mut Connection,
    application_id: &ApplicationId,
    branch: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    events: &mut dyn FnMut(DeploymentEvent),
) -> Result<DeploymentResult, DeployBranchError> {
    let mut events = EventReporter::enabled(events);
    deploy_branch_reporting(
        connection,
        application_id,
        branch,
        public_configuration,
        &mut events,
    )
}

#[derive(Debug, Error)]
pub enum DeployOciError {
    #[error("failed to acquire deployment lock: {source}")]
    ApplicationLock {
        #[source]
        source: ApplicationLockError,
    },
    #[error("application `{application_id}` already has an operation in progress")]
    ApplicationBusy { application_id: String },
    #[error(
        "application `{application_id}` has no delivery configuration; re-import its manifest with a [delivery] section"
    )]
    NoDeliveryConfiguration { application_id: String },
    #[error("application `{application_id}` only accepts images from `{allowed}`, not `{actual}`")]
    RepositoryMismatch {
        application_id: String,
        allowed: String,
        actual: String,
    },
    #[error("failed to load delivery configuration: {source}")]
    DeliveryConfiguration {
        #[source]
        source: ApplicationStoreError,
    },
    #[error("failed to pull OCI image: {source}")]
    PullImage {
        #[source]
        source: PullImageError,
    },
    #[error(transparent)]
    CreateRelease { source: CreateReleaseError },
    #[error(transparent)]
    DeployRelease { source: DeployReleaseError },
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
    let mut events = EventReporter::disabled();
    deploy_oci_reporting(
        connection,
        application_id,
        artifact,
        source_commit,
        public_configuration,
        &mut events,
    )
}

fn deploy_oci_reporting(
    connection: &mut Connection,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    events: &mut EventReporter<'_>,
) -> Result<DeploymentResult, DeployOciError> {
    let Some(_lock) = ApplicationLock::try_acquire_for_connection(connection, application_id)
        .map_err(|source| DeployOciError::ApplicationLock { source })?
    else {
        return Err(DeployOciError::ApplicationBusy {
            application_id: application_id.to_string(),
        });
    };
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
        events,
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
    events: &mut EventReporter<'_>,
) -> Result<DeploymentResult, DeployOciError> {
    if !delivery.permits(artifact) {
        return Err(DeployOciError::RepositoryMismatch {
            application_id: application_id.to_string(),
            allowed: delivery.image_repository().as_str().to_owned(),
            actual: artifact.repository().to_owned(),
        });
    }
    events.started(DeploymentStep::PullImage);
    pull_image(artifact).map_err(|source| DeployOciError::PullImage { source })?;
    events.completed(DeploymentStep::PullImage);
    let release = create_release_while_locked(connection, application_id, artifact)
        .map_err(|source| DeployOciError::CreateRelease { source })?;
    let source_revision = source_commit.cloned();
    deploy_release_reporting(
        connection,
        application_id,
        &release,
        DeploymentType::Deploy,
        source_revision.as_ref(),
        public_configuration,
        events,
    )
    .map_err(|source| DeployOciError::DeployRelease { source })
}

// Deploys an OCI artifact while forwarding semantic workflow events to the caller.
pub fn deploy_oci_with_events(
    connection: &mut Connection,
    application_id: &ApplicationId,
    artifact: &OciArtifact,
    source_commit: Option<&CommitSha>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    events: &mut dyn FnMut(DeploymentEvent),
) -> Result<DeploymentResult, DeployOciError> {
    let mut events = EventReporter::enabled(events);
    deploy_oci_reporting(
        connection,
        application_id,
        artifact,
        source_commit,
        public_configuration,
        &mut events,
    )
}
