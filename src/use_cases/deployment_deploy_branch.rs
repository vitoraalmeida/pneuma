use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::git_source::{CommitSha, ResolveBranchError, resolve_branch};
use crate::adapters::oci_image::{ResolveImageDigestError, resolve_image_digest};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::use_cases::deployment_deploy_oci::{DeployOciError, deploy_oci};
use crate::use_cases::deployment_deploy_release::{
    DeploymentResult, PublicDeploymentConfiguration,
};

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

pub fn deploy_branch(
    connection: &mut Connection,
    application_id: &str,
    branch: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, DeployBranchError> {
    let (repository_url, default_branch) =
        application_store::load_source_repository(connection, application_id)
            .map_err(|source| DeployBranchError::SourceConfiguration { source })?
            .ok_or_else(|| DeployBranchError::NoSourceConfiguration {
                application_id: application_id.to_owned(),
            })?;

    let branch = match branch {
        Some(branch) => branch.to_owned(),
        None => default_branch.ok_or_else(|| DeployBranchError::NoDefaultBranch {
            application_id: application_id.to_owned(),
        })?,
    };

    let commit_sha: CommitSha = resolve_branch(&repository_url, &branch)
        .map_err(|source| DeployBranchError::ResolveBranch { source })?;

    let image_repository =
        application_store::load_delivery_image_repository(connection, application_id)
            .map_err(|source| DeployBranchError::SourceConfiguration { source })?
            .ok_or_else(|| DeployBranchError::NoDeliveryConfiguration {
                application_id: application_id.to_owned(),
            })?;

    let reference = resolve_image_digest(&image_repository, &commit_sha)
        .map_err(|source| DeployBranchError::ResolveImageDigest { source })?;

    deploy_oci(
        connection,
        application_id,
        reference.as_str(),
        Some(commit_sha.as_str()),
        public_configuration,
    )
    .map_err(|source| DeployBranchError::DeployOci { source })
}
