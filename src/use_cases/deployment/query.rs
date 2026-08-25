use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::domain::deployment::DeploymentHistory;
use crate::domain::identity::ApplicationId;

// Reads typed deployment history; SQL mapping remains owned by the deployment store.
pub fn list_deployments(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Vec<DeploymentHistory>, ListDeploymentsError> {
    deployment_store::list_deployment_history(connection, application_id)
        .map_err(ListDeploymentsError::Store)
}

#[derive(Debug, Error)]
pub enum ListDeploymentsError {
    #[error("failed to list deployments: {0}")]
    Store(#[source] DeploymentStoreError),
}
