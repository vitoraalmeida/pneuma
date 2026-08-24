use rusqlite::Connection;

use crate::adapters::stores::{
    application_store, deployment_store, exposure_store, release_store, runtime_store,
};
use crate::domain::application::ApplicationName;
use crate::domain::reconciliation::{
    ActiveRuntime, DesiredState, PersistedState, ReconciliationInput,
};

use super::ReconciliationReadError;

pub(crate) fn persistence_error(source: rusqlite::Error) -> ReconciliationReadError {
    ReconciliationReadError::Application {
        source: application_store::ApplicationStoreError::Persistence { source },
    }
}

// Loads desired intent and persisted bookkeeping from SQLite in one short read transaction before external observation.
pub fn load_reconciliation_input(
    connection: &mut Connection,
    application_name: &ApplicationName,
) -> Result<ReconciliationInput, ReconciliationReadError> {
    let transaction = connection.transaction().map_err(persistence_error)?;
    let application = application_store::load_application_by_name(&transaction, application_name)
        .map_err(|source| ReconciliationReadError::Application { source })?
        .ok_or_else(|| ReconciliationReadError::ApplicationNotFound {
            application_name: application_name.as_str().to_owned(),
        })?;
    let blocking_deployment =
        deployment_store::load_nonterminal_deployment(&transaction, &application.id)
            .map_err(|source| ReconciliationReadError::Deployment { source })?;
    let exposure = exposure_store::load_exposure(&transaction, &application.id)
        .map_err(|source| ReconciliationReadError::Exposure { source })?;
    let specification =
        application_store::load_deployment_specification(&transaction, &application.id)
            .map_err(|source| ReconciliationReadError::Application { source })?;
    let active = match &application.active_deployment_id {
        Some(deployment_id) => {
            let deployment = deployment_store::load_deployment(&transaction, deployment_id)
                .map_err(|source| ReconciliationReadError::Deployment { source })?;
            let release = release_store::load_release_by_id(&transaction, &deployment.release_id)
                .map_err(|source| ReconciliationReadError::Release { source })?;
            let runtime =
                runtime_store::load_active_successful_runtime(&transaction, &application.id)
                    .map_err(|source| ReconciliationReadError::Runtime { source })?;
            Some(ActiveRuntime {
                deployment,
                release,
                runtime,
            })
        }
        None => None,
    };
    transaction.commit().map_err(persistence_error)?;
    Ok(ReconciliationInput {
        desired: DesiredState {
            application,
            exposure,
        },
        persisted: PersistedState {
            blocking_deployment,
            active,
            specification,
        },
    })
}
