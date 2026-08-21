use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::oci_image::{PullImageError, pull_image};
use crate::adapters::stores::application_store;
use crate::adapters::stores::deployment_store;
use crate::domain::deployment::{DeploymentType, SourceRevision};
use crate::domain::identity::ApplicationId;
use crate::use_cases::deployment_execute_release::{
    DeployReleaseError, DeploymentResult, PublicDeploymentConfiguration, deploy_release,
};

#[derive(Debug)]
pub enum RollbackError {
    ApplicationNotFound {
        application_id: String,
    },
    NoPreviousDeployment {
        application_id: String,
    },
    ApplicationStore {
        source: application_store::ApplicationStoreError,
    },
    DeploymentStore {
        source: deployment_store::DeploymentStoreError,
    },
    Persistence {
        source: rusqlite::Error,
    },
    PullImage {
        source: PullImageError,
    },
    DeployRelease {
        source: DeployReleaseError,
    },
}

impl fmt::Display for RollbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::NoPreviousDeployment { application_id } => write!(
                formatter,
                "application `{application_id}` has no previous successful deployment to roll back to"
            ),
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to load rollback release: {source}")
            }
            Self::DeploymentStore { source } => {
                write!(formatter, "failed to load rollback release: {source}")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to load rollback release: {source}")
            }
            Self::PullImage { source } => {
                write!(formatter, "failed to pull rollback image: {source}")
            }
            Self::DeployRelease { source } => write!(formatter, "{source}"),
        }
    }
}

impl Error for RollbackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::ApplicationStore { source } => Some(source),
            Self::DeploymentStore { source } => Some(source),
            Self::PullImage { source } => Some(source),
            Self::DeployRelease { source } => Some(source),
            Self::ApplicationNotFound { .. } | Self::NoPreviousDeployment { .. } => None,
        }
    }
}

// Reuses a historical immutable artifact through the normal deployment flow, preserving history.
pub fn rollback_deployment(
    connection: &mut Connection,
    application_id: &ApplicationId,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, RollbackError> {
    let exists = application_store::application_exists(connection, application_id.as_str())
        .map_err(|source| RollbackError::ApplicationStore { source })?;
    if !exists {
        return Err(RollbackError::ApplicationNotFound {
            application_id: application_id.to_string(),
        });
    }
    let target = previous_release(connection, application_id.as_str())?;
    pull_image(target.release.artifact.reference())
        .map_err(|source| RollbackError::PullImage { source })?;
    deploy_release(
        connection,
        application_id,
        &target.release,
        DeploymentType::Rollback,
        target.source_revision.as_ref().map(SourceRevision::as_str),
        public_configuration,
    )
    .map_err(|source| RollbackError::DeployRelease { source })
}

// Selects the newest succeeded deployment that is not currently active.
type RollbackTarget = deployment_store::RollbackTarget;

fn previous_release(
    connection: &Connection,
    application_id: &str,
) -> Result<RollbackTarget, RollbackError> {
    deployment_store::load_rollback_target(connection, application_id)
        .map_err(|source| RollbackError::DeploymentStore { source })?
        .ok_or_else(|| RollbackError::NoPreviousDeployment {
            application_id: application_id.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::adapters::database;
    use crate::domain::deployment::SourceRevision;

    use super::previous_release;

    #[test]
    fn selects_provenance_from_the_historical_deployment() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO applications (
                    id, name, desired_runtime_state, spec_version, created_at, updated_at
                 ) VALUES ('app-id', 'app', 'stopped', 1, 'now', 'now');
                 INSERT INTO releases (
                    id, application_id, image_repository, image_digest, image_reference, created_at
                 ) VALUES (
                    'release-id', 'app-id', 'registry.example/app',
                    'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'now'
                 );
                 INSERT INTO deployments (
                    id, application_id, release_id, type, status, source_revision,
                    requested_at, finished_at
                 ) VALUES (
                    'deployment-id', 'app-id', 'release-id', 'deploy', 'succeeded',
                    'historical-commit', 'now', 'now'
                 );",
            )
            .unwrap();

        let target = previous_release(&connection, "app-id").unwrap();

        assert_eq!(target.release.id.as_str(), "release-id");
        assert_eq!(target.release.artifact.repository(), "registry.example/app");
        assert_eq!(
            target.source_revision.as_ref().map(SourceRevision::as_str),
            Some("historical-commit")
        );
    }
}
