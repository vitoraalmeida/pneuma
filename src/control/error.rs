use thiserror::Error;

use crate::adapters::database::DatabaseError;
use crate::adapters::stores::application_store::ApplicationStoreError;
use crate::adapters::stores::deployment_store::DeploymentStoreError;
use crate::domain::release::InvalidOciArtifact;
use crate::domain::system::InvalidSystemName;
use crate::use_cases::application::{
    ApplicationLookupError, RemoteImportError, RuntimeLifecycleError,
};
use crate::use_cases::deployment::{DeployBranchError, DeployOciError, RollbackError};
use crate::use_cases::exposure::ExposureChangeError;
use crate::use_cases::reconciliation::ReconciliationReadError;
use crate::use_cases::system::ShowError;

/// Typed failure of one executed command. Messages stay command-specific so
/// adapters can present them verbatim.
#[derive(Debug, Error)]
pub enum ControlError {
    #[error(transparent)]
    Database { source: DatabaseError },
    #[error(transparent)]
    InvalidSystemName { source: InvalidSystemName },
    #[error("failed to create system: {source}")]
    SystemCreate {
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to list systems: {source}")]
    SystemList {
        #[source]
        source: rusqlite::Error,
    },
    #[error(transparent)]
    SystemShow { source: ShowError },
    #[error(transparent)]
    Import { source: RemoteImportError },
    #[error("failed to list applications: {source}")]
    ListApplications {
        #[source]
        source: ApplicationStoreError,
    },
    #[error(transparent)]
    ApplicationLookup { source: ApplicationLookupError },
    #[error("failed to list deployments: {source}")]
    ListDeployments {
        #[source]
        source: DeploymentStoreError,
    },
    #[error(transparent)]
    RuntimeStatus { source: RuntimeLifecycleError },
    #[error(transparent)]
    RuntimeStop { source: RuntimeLifecycleError },
    #[error(transparent)]
    RuntimeStart { source: RuntimeLifecycleError },
    #[error(transparent)]
    InvalidOciArtifact { source: InvalidOciArtifact },
    #[error(transparent)]
    DeployOci { source: DeployOciError },
    #[error(transparent)]
    DeployBranch { source: DeployBranchError },
    #[error(transparent)]
    Rollback { source: RollbackError },
    #[error(transparent)]
    VisibilitySet { source: ExposureChangeError },
    #[error(transparent)]
    Reconcile { source: ReconciliationReadError },
}
