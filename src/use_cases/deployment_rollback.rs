use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension};

use crate::adapters::oci_image::{OciImageReference, PullImageError, pull_image};
use crate::domain::deployment::DeploymentType;
use crate::domain::release::Release;
use crate::use_cases::deployment_deploy_release::{
    DeployReleaseError, DeployedRelease, PublicDeploymentConfiguration, deploy_release,
};

#[derive(Debug)]
pub enum RollbackError {
    ApplicationNotFound { application_id: String },
    NoPreviousDeployment { application_id: String },
    Persistence { source: rusqlite::Error },
    PullImage { source: PullImageError },
    DeployRelease { source: DeployReleaseError },
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
            Self::PullImage { source } => Some(source),
            Self::DeployRelease { source } => Some(source),
            Self::ApplicationNotFound { .. } | Self::NoPreviousDeployment { .. } => None,
        }
    }
}

pub fn rollback_deployment(
    connection: &mut Connection,
    application_id: &str,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeployedRelease, RollbackError> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM applications WHERE id = ?1)",
            [application_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|source| RollbackError::Persistence { source })?;
    if !exists {
        return Err(RollbackError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        });
    }
    let (release, source_revision) = previous_release(connection, application_id)?;
    if OciImageReference::parse(&release.image_reference).is_ok() {
        pull_image(&release.image_reference)
            .map_err(|source| RollbackError::PullImage { source })?;
    }
    deploy_release(
        connection,
        application_id,
        &release,
        DeploymentType::Rollback,
        source_revision.as_deref(),
        public_configuration,
    )
    .map_err(|source| RollbackError::DeployRelease { source })
}

fn previous_release(
    connection: &Connection,
    application_id: &str,
) -> Result<(Release, Option<String>), RollbackError> {
    connection
        .query_row(
            "SELECT r.id, r.application_id, r.image_reference, r.image_repository,
                    r.image_digest, d.source_revision, r.created_at
             FROM deployments d
             JOIN releases r ON r.id = d.release_id
             LEFT JOIN applications a ON a.active_deployment_id = d.id
             WHERE d.application_id = ?1
               AND d.status = 'succeeded'
               AND a.id IS NULL
             ORDER BY d.finished_at DESC
             LIMIT 1",
            [application_id],
            |row| {
                Ok((
                    Release {
                        id: row.get(0)?,
                        application_id: row.get(1)?,
                        image_reference: row.get(2)?,
                        image_repository: row.get(3)?,
                        image_digest: row.get(4)?,
                        created_at: row.get(6)?,
                    },
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|source| RollbackError::Persistence { source })?
        .ok_or_else(|| RollbackError::NoPreviousDeployment {
            application_id: application_id.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::adapters::database;

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
                    'release-id', 'app-id', 'registry.example/app', 'sha256:artifact',
                    'registry.example/app@sha256:artifact', 'now'
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

        let (release, source_revision) = previous_release(&connection, "app-id").unwrap();

        assert_eq!(release.id, "release-id");
        assert_eq!(source_revision.as_deref(), Some("historical-commit"));
    }
}
