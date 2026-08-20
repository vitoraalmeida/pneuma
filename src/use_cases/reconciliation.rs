use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::caddy_exposure::{ObserveCaddyFragmentError, observe_caddy_fragment};
use crate::adapters::local_runtime::{
    ObserveContainerError, ObserveNamedContainerError, observe_container, observe_named_container,
};
use crate::adapters::stores::{application_store, deployment_store, release_store, runtime_store};
use crate::adapters::systemd_quadlet::{
    QuadletError, container_name, observe_generated_unit, observe_unit_source, unit_name,
};
use crate::domain::deployment::Deployment;
use crate::domain::reconciliation::{
    ActiveRuntime, CaddyFragmentObservation, NamedContainerObservation, ReconciliationInput,
    ReconciliationObservation,
};
use crate::domain::runtime::{DesiredRuntimeState, ObservedRuntimeState};

#[derive(Debug)]
pub enum ReconciliationResult {
    NoOp,
    Deferred {
        blocking_deployment: Box<Deployment>,
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
        source: application_store::ExposureStoreError,
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
            Self::ObserveContainer { source } => Some(source),
            Self::ObserveNamedContainer { source } => Some(source),
            Self::ObserveQuadlet { source } => Some(source),
            Self::ObserveCaddy { source } => Some(source),
            Self::ApplicationNotFound { .. } | Self::NotConverged { .. } => None,
        }
    }
}

// Returns a read-only result for states that already satisfy persisted intent; later checkpoints repair drift.
pub fn reconcile_application(
    connection: &mut Connection,
    application_name: &str,
    managed_caddy_directory: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let input = load_reconciliation_input(connection, application_name)?;
    if let Some(blocking_deployment) = input.blocking_deployment {
        return Ok(ReconciliationResult::Deferred {
            blocking_deployment: Box::new(blocking_deployment),
        });
    }
    let observation = observe_reconciliation_input(&input, managed_caddy_directory)?;
    let Some(observation) = observation else {
        return Err(ReconciliationReadError::NotConverged {
            reason: "application has no active runtime".to_owned(),
        });
    };
    if input.application.desired_runtime_state == DesiredRuntimeState::Stopped
        && *observation.recorded_container.state() == ObservedRuntimeState::Missing
        && observation.named_container == NamedContainerObservation::Missing
        && observation.caddy_fragment == CaddyFragmentObservation::Missing
    {
        return Ok(ReconciliationResult::NoOp);
    }
    Err(ReconciliationReadError::NotConverged {
        reason: "runtime repair and public-route confirmation are not implemented".to_owned(),
    })
}

// Loads all persisted reconciliation authorities in a short read transaction before external observation.
pub fn load_reconciliation_input(
    connection: &mut Connection,
    application_name: &str,
) -> Result<ReconciliationInput, ReconciliationReadError> {
    let transaction =
        connection
            .transaction()
            .map_err(|source| ReconciliationReadError::Application {
                source: application_store::ApplicationStoreError::Persistence { source },
            })?;
    let application = application_store::load_application_by_name(&transaction, application_name)
        .map_err(|source| ReconciliationReadError::Application { source })?
        .ok_or_else(|| ReconciliationReadError::ApplicationNotFound {
            application_name: application_name.to_owned(),
        })?;
    let blocking_deployment =
        deployment_store::load_nonterminal_deployment(&transaction, application.id.as_str())
            .map_err(|source| ReconciliationReadError::Deployment { source })?;
    let exposure = application_store::load_exposure(&transaction, application.id.as_str())
        .map_err(|source| ReconciliationReadError::Exposure { source })?;
    let specification =
        application_store::load_deployment_specification(&transaction, application.id.as_str())
            .map_err(|source| ReconciliationReadError::Application { source })?;
    let active = match &application.active_deployment_id {
        Some(deployment_id) => {
            let deployment =
                deployment_store::load_deployment(&transaction, deployment_id.as_str())
                    .map_err(|source| ReconciliationReadError::Deployment { source })?;
            let release =
                release_store::load_release_by_id(&transaction, deployment.release_id.as_str())
                    .map_err(|source| ReconciliationReadError::Release { source })?;
            let runtime = runtime_store::load_current_successful_runtime(
                &transaction,
                application.id.as_str(),
            )
            .map_err(|source| ReconciliationReadError::Runtime { source })?;
            Some(ActiveRuntime {
                deployment,
                release,
                runtime,
            })
        }
        None => None,
    };
    transaction
        .commit()
        .map_err(|source| ReconciliationReadError::Application {
            source: application_store::ApplicationStoreError::Persistence { source },
        })?;
    Ok(ReconciliationInput {
        application,
        blocking_deployment,
        active,
        exposure,
        specification,
    })
}

// Observes persisted runtime and route identities without changing SQLite or controlling external resources.
pub fn observe_reconciliation_input(
    input: &ReconciliationInput,
    managed_caddy_directory: &std::path::Path,
) -> Result<Option<ReconciliationObservation>, ReconciliationReadError> {
    let Some(active) = &input.active else {
        return Ok(None);
    };
    let Some(runtime) = &active.runtime else {
        return Ok(None);
    };
    let recorded_container =
        observe_container(runtime.external_runtime_id.as_str(), runtime.container_port)
            .map_err(|source| ReconciliationReadError::ObserveContainer { source })?;
    let name = container_name(
        input.application.name.as_str(),
        active.deployment.id.as_str(),
    );
    let named_container = observe_named_container(&name, runtime.container_port)
        .map_err(|source| ReconciliationReadError::ObserveNamedContainer { source })?;
    let unit = unit_name(
        input.application.name.as_str(),
        active.deployment.id.as_str(),
    );
    let quadlet_source = observe_unit_source(&unit)
        .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    let systemd_unit = observe_generated_unit(&unit)
        .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    let caddy_fragment =
        observe_caddy_fragment(managed_caddy_directory, input.application.id.as_str())
            .map_err(|source| ReconciliationReadError::ObserveCaddy { source })?;
    Ok(Some(ReconciliationObservation {
        recorded_container,
        named_container,
        quadlet_source,
        systemd_unit,
        caddy_fragment,
    }))
}
