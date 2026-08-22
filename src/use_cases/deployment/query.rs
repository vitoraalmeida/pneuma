use std::error::Error;
use std::fmt;

use rusqlite::Connection;

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

#[derive(Debug)]
pub enum ListDeploymentsError {
    Store(DeploymentStoreError),
}

impl fmt::Display for ListDeploymentsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(source) => write!(formatter, "failed to list deployments: {source}"),
        }
    }
}

impl Error for ListDeploymentsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(source) => Some(source),
        }
    }
}
