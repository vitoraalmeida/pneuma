use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::application_lock::{ApplicationLock, ApplicationLockError};
use crate::adapters::caddy_exposure::ObserveCaddyFragmentError;
use crate::adapters::local_runtime::{ObserveContainerError, ObserveNamedContainerError};
use crate::adapters::stores::operation_store::{self, OperationStoreError};
use crate::adapters::stores::{
    application_store, deployment_store, exposure_store, release_store, runtime_store,
};
use crate::adapters::systemd_quadlet::QuadletError;
use crate::adapters::test_gate::wait_for_test_gate;
use crate::domain::application::ApplicationName;
use crate::domain::deployment::Deployment;
use crate::domain::reconciliation::{ActiveRuntime, ReconciliationInput, decide};
use crate::domain::runtime::{InvalidHostPort, RuntimeInstance};

mod execute;
mod load;
mod observe;
mod recover;

pub use load::load_reconciliation_input;
pub(crate) use observe::observe_reconciliation_input;

use execute::{execute_reconciliation_decision, reconciliation_decision_reason};
use load::persistence_error;
use observe::reconciliation_expectations;
use recover::recover_interrupted_deployment;

#[derive(Debug)]
pub enum ReconciliationResult {
    NoOp,
    Deferred {
        blocking_deployment: Option<Box<Deployment>>,
    },
    Repaired {
        runtime_id: String,
        container_id: String,
    },
    ManualIntervention {
        reason: String,
    },
    ExposureRepaired,
    Failed {
        reason: String,
    },
    Diverged {
        reason: String,
    },
}

#[derive(Debug)]
pub enum ReconciliationReadError {
    ApplicationNotFound {
        application_name: String,
    },
    Application {
        source: application_store::ApplicationStoreError,
    },
    Deployment {
        source: deployment_store::DeploymentStoreError,
    },
    Release {
        source: release_store::ReleaseStoreError,
    },
    Runtime {
        source: runtime_store::RuntimeStoreError,
    },
    Exposure {
        source: exposure_store::ExposureStoreError,
    },
    OperationLock {
        source: ApplicationLockError,
    },
    Operation {
        source: OperationStoreError,
    },
    ObserveContainer {
        source: ObserveContainerError,
    },
    ObserveNamedContainer {
        source: ObserveNamedContainerError,
    },
    ObserveQuadlet {
        source: QuadletError,
    },
    ObserveCaddy {
        source: ObserveCaddyFragmentError,
    },
    InvalidExpectedPort {
        source: InvalidHostPort,
    },
    NotConverged {
        reason: String,
    },
}

impl fmt::Display for ReconciliationReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_name } => {
                write!(formatter, "application `{application_name}` was not found")
            }
            Self::Application { source } => write!(
                formatter,
                "failed to load reconciliation application: {source}"
            ),
            Self::Deployment { source } => write!(
                formatter,
                "failed to load reconciliation deployment: {source}"
            ),
            Self::Release { source } => {
                write!(formatter, "failed to load reconciliation release: {source}")
            }
            Self::Runtime { source } => {
                write!(formatter, "failed to load reconciliation runtime: {source}")
            }
            Self::Exposure { source } => write!(
                formatter,
                "failed to load reconciliation exposure: {source}"
            ),
            Self::OperationLock { source } => {
                write!(formatter, "failed to serialize reconciliation: {source}")
            }
            Self::Operation { source } => {
                write!(
                    formatter,
                    "failed to acquire reconciliation ownership: {source}"
                )
            }
            Self::ObserveContainer { source } => {
                write!(formatter, "failed to observe recorded runtime: {source}")
            }
            Self::ObserveNamedContainer { source } => {
                write!(formatter, "failed to observe named runtime: {source}")
            }
            Self::ObserveQuadlet { source } => {
                write!(formatter, "failed to observe Quadlet: {source}")
            }
            Self::ObserveCaddy { source } => {
                write!(formatter, "failed to observe Caddy fragment: {source}")
            }
            Self::InvalidExpectedPort { source } => write!(
                formatter,
                "runtime has an invalid expected host port: {source}"
            ),
            Self::NotConverged { reason } => {
                write!(formatter, "reconciliation is not yet converged: {reason}")
            }
        }
    }
}

impl Error for ReconciliationReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Application { source } => Some(source),
            Self::Deployment { source } => Some(source),
            Self::Release { source } => Some(source),
            Self::Runtime { source } => Some(source),
            Self::Exposure { source } => Some(source),
            Self::OperationLock { source } => Some(source),
            Self::Operation { source } => Some(source),
            Self::ObserveContainer { source } => Some(source),
            Self::ObserveNamedContainer { source } => Some(source),
            Self::ObserveQuadlet { source } => Some(source),
            Self::ObserveCaddy { source } => Some(source),
            Self::InvalidExpectedPort { source } => Some(source),
            Self::ApplicationNotFound { .. } | Self::NotConverged { .. } => None,
        }
    }
}

// Reconciles only confirmed runtime and route drift, leaving ambiguous materialization untouched.
//
// Pipeline: acquire ownership, load persisted facts, observe external facts,
// decide purely, then execute exactly the decided variant. Each phase owns its
// own adapter details; this entrypoint coordinates ordering and compensation only.
pub fn reconcile_application(
    connection: &mut Connection,
    application_name: &ApplicationName,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let application = application_store::load_application_by_name(connection, application_name)
        .map_err(|source| ReconciliationReadError::Application { source })?
        .ok_or_else(|| ReconciliationReadError::ApplicationNotFound {
            application_name: application_name.as_str().to_owned(),
        })?;
    let database_path = connection.path().map(std::path::PathBuf::from).ok_or(
        ReconciliationReadError::OperationLock {
            source: ApplicationLockError::DatabasePathUnavailable,
        },
    )?;
    let Some(_lock) = ApplicationLock::try_acquire(&database_path, &application.id)
        .map_err(|source| ReconciliationReadError::OperationLock { source })?
    else {
        return Ok(ReconciliationResult::Deferred {
            blocking_deployment: None,
        });
    };
    let token = operation_store::generate_token(connection)
        .map_err(|source| ReconciliationReadError::Operation { source })?;
    let transaction = connection.transaction().map_err(persistence_error)?;
    operation_store::take_ownership(&transaction, &application.id, &token)
        .map_err(|source| ReconciliationReadError::Operation { source })?;
    transaction.commit().map_err(persistence_error)?;
    wait_for_test_gate("reconciliation.ownership-acquired").map_err(|source| {
        ReconciliationReadError::NotConverged {
            reason: format!("reconciliation test gate failed: {source}"),
        }
    })?;

    let input = load_reconciliation_input(connection, application_name)?;
    if let Some(blocking_deployment) = input.persisted.blocking_deployment {
        return recover_interrupted_deployment(
            connection,
            &input.desired.application,
            input.persisted.active.as_ref(),
            input.desired.exposure.as_ref(),
            &blocking_deployment,
            managed_caddy_directory,
        );
    }
    let Some(observation) = observe_reconciliation_input(&input, managed_caddy_directory)? else {
        return Err(ReconciliationReadError::NotConverged {
            reason: "application has no active runtime".to_owned(),
        });
    };
    let expectations = reconciliation_expectations(&input)?;
    let decision = decide(&input, &observation, &expectations).map_err(|error| {
        ReconciliationReadError::NotConverged {
            reason: reconciliation_decision_reason(error),
        }
    })?;
    execute_reconciliation_decision(
        connection,
        &input,
        &expectations,
        decision,
        managed_caddy_directory,
        caddyfile_path,
    )
}

// Returns the confirmed active deployment and its retained runtime identity.
pub(crate) fn required_active_runtime(
    input: &ReconciliationInput,
) -> Result<(&ActiveRuntime, &RuntimeInstance), ReconciliationReadError> {
    let missing_runtime = || ReconciliationReadError::NotConverged {
        reason: "application has no active runtime".to_owned(),
    };
    let active = input
        .persisted
        .active
        .as_ref()
        .ok_or_else(missing_runtime)?;
    let runtime = active.runtime.as_ref().ok_or_else(missing_runtime)?;
    Ok((active, runtime))
}

pub(crate) fn host_port(
    runtime: &RuntimeInstance,
) -> Result<crate::domain::runtime::HostPort, ReconciliationReadError> {
    runtime
        .expected_endpoint
        .host_port()
        .map_err(|source| ReconciliationReadError::InvalidExpectedPort { source })
}

// Marks an executor precondition the pure decision already guaranteed; refusing
// to guess keeps impossible states explicit instead of silently proceeding.
pub(crate) fn inconsistent_input(reason: &'static str) -> ReconciliationReadError {
    ReconciliationReadError::NotConverged {
        reason: reason.to_owned(),
    }
}
