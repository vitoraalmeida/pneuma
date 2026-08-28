use rusqlite::Connection;

use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::domain::deployment::DeploymentHistory;
use crate::domain::identity::ApplicationId;

// Reads typed deployment history; SQL mapping remains owned by the deployment store.
pub fn list_deployments(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Vec<DeploymentHistory>, DeploymentStoreError> {
    deployment_store::list_deployment_history(connection, application_id)
}
