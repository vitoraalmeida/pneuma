use rusqlite::Connection;
use thiserror::Error;

use super::execute::{DeploymentResult, PublicDeploymentConfiguration, deploy_release};
use super::failure::DeployReleaseError;
use crate::adapters::oci_image::{PullImageError, pull_image};
use crate::adapters::stores::application_store;
use crate::adapters::stores::deployment_store;
use crate::domain::deployment::{DeploymentType, RollbackTarget};
use crate::domain::identity::ApplicationId;

#[derive(Debug, Error)]
pub enum RollbackError {
    #[error("application `{application_id}` was not found")]
    ApplicationNotFound { application_id: String },
    #[error("application `{application_id}` has no previous successful deployment to roll back to")]
    NoPreviousDeployment { application_id: String },
    #[error("failed to load rollback release: {source}")]
    ApplicationStore {
        #[source]
        source: application_store::ApplicationStoreError,
    },
    #[error("failed to load rollback release: {source}")]
    DeploymentStore {
        #[source]
        source: deployment_store::DeploymentStoreError,
    },
    #[error("failed to load rollback release: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to pull rollback image: {source}")]
    PullImage {
        #[source]
        source: PullImageError,
    },
    #[error(transparent)]
    DeployRelease { source: DeployReleaseError },
}

// Reuses a historical immutable artifact through the normal deployment flow, preserving history.
pub fn rollback_deployment(
    connection: &mut Connection,
    application_id: &ApplicationId,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, RollbackError> {
    let exists = application_store::application_exists(connection, application_id)
        .map_err(|source| RollbackError::ApplicationStore { source })?;
    if !exists {
        return Err(RollbackError::ApplicationNotFound {
            application_id: application_id.to_string(),
        });
    }
    let target = previous_release(connection, application_id)?;
    pull_image(&target.release.artifact).map_err(|source| RollbackError::PullImage { source })?;
    deploy_release(
        connection,
        application_id,
        &target.release,
        DeploymentType::Rollback,
        target.source_revision.as_ref(),
        public_configuration,
    )
    .map_err(|source| RollbackError::DeployRelease { source })
}

// Selects the newest succeeded deployment that is not currently active.
fn previous_release(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<RollbackTarget, RollbackError> {
    deployment_store::load_rollback_target(connection, application_id)
        .map_err(|source| RollbackError::DeploymentStore { source })?
        .ok_or_else(|| RollbackError::NoPreviousDeployment {
            application_id: application_id.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::adapters::database;
    use crate::domain::deployment::SourceRevision;
    use crate::domain::identity::ApplicationId;

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

        let target = previous_release(&connection, &ApplicationId::from("app-id")).unwrap();

        assert_eq!(target.release.id.as_str(), "release-id");
        assert_eq!(target.release.artifact.repository(), "registry.example/app");
        assert_eq!(
            target.source_revision.as_ref().map(SourceRevision::as_str),
            Some("historical-commit")
        );
    }
}
