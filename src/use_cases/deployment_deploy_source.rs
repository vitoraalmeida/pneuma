use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use rusqlite::Connection;

use crate::adapters::git_source::{ResolveCommitError, create_checkout, resolve_commit};
use crate::adapters::local_build::build_image;
use crate::use_cases::deployment_create::DeploymentType;
use crate::use_cases::deployment_deploy_release::{
    DeployReleaseError, DeployedRelease, PublicDeploymentConfiguration, deploy_release,
};
use crate::use_cases::release_create::{CreateReleaseError, create_release};

#[derive(Debug)]
pub enum DeploySourceError {
    ResolveRevision { source: ResolveCommitError },
    PrepareCheckout { source: Box<dyn Error> },
    BuildImage { source: Box<dyn Error> },
    CreateRelease { source: CreateReleaseError },
    DeployRelease { source: DeployReleaseError },
}

impl fmt::Display for DeploySourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResolveRevision { source } => write!(formatter, "{source}"),
            Self::PrepareCheckout { source } => {
                write!(formatter, "failed to prepare source: {source}")
            }
            Self::BuildImage { source } => write!(formatter, "failed to build image: {source}"),
            Self::CreateRelease { source } => write!(formatter, "{source}"),
            Self::DeployRelease { source } => write!(formatter, "{source}"),
        }
    }
}

impl Error for DeploySourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ResolveRevision { source } => Some(source),
            Self::PrepareCheckout { source } | Self::BuildImage { source } => Some(source.as_ref()),
            Self::CreateRelease { source } => Some(source),
            Self::DeployRelease { source } => Some(source),
        }
    }
}

pub fn deploy_source(
    connection: &mut Connection,
    application_id: &str,
    repository_path: &Path,
    revision: &str,
    workspace_root: &Path,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeployedRelease, DeploySourceError> {
    let (application_name, containerfile, context) = connection
        .query_row(
            "SELECT applications.name, application_build_specs.containerfile_path,
                    application_build_specs.context_path
             FROM applications
             JOIN application_build_specs ON application_build_specs.application_id = applications.id
             WHERE applications.id = ?1",
            [application_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        )
        .map_err(|source| DeploySourceError::PrepareCheckout { source: Box::new(source) })?;
    let commit_sha = resolve_commit(repository_path, revision)
        .map_err(|source| DeploySourceError::ResolveRevision { source })?;
    let checkout_path = workspace_root.join(format!("source-{commit_sha}"));
    fs::create_dir_all(workspace_root).map_err(|source| DeploySourceError::PrepareCheckout {
        source: Box::new(source),
    })?;
    create_checkout(repository_path, &commit_sha, &checkout_path).map_err(|source| {
        DeploySourceError::PrepareCheckout {
            source: Box::new(source),
        }
    })?;
    let image = build_image(
        &checkout_path,
        &application_name,
        &commit_sha,
        Path::new(&containerfile),
        Path::new(&context),
    )
    .map_err(|source| DeploySourceError::BuildImage {
        source: Box::new(source),
    })?;
    let image_repository = format!("localhost/pneuma/{application_name}");
    let release = create_release(
        connection,
        application_id,
        &image.reference,
        &image_repository,
        &commit_sha,
        Some(&commit_sha),
    )
    .map_err(|source| DeploySourceError::CreateRelease { source })?;
    deploy_release(
        connection,
        application_id,
        &release,
        DeploymentType::Deploy,
        public_configuration,
    )
    .map_err(|source| DeploySourceError::DeployRelease { source })
}
