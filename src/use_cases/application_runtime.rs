use std::error::Error;
use std::fmt;
use std::net::SocketAddr;

use rusqlite::Connection;

use crate::adapters::local_runtime::{
    ContainerCommandOutput, ControlContainerError, ObserveContainerError, observe_container,
    resolve_container_id, start_container, stop_container,
};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::adapters::systemd_quadlet::{
    QuadletError, container_name, start as start_unit, stop as stop_unit, unit_exists, unit_name,
};
use crate::domain::application::DesiredRuntimeState;
use crate::domain::identity::{ApplicationId, ContainerId, RuntimeInstanceId};
use crate::domain::runtime::{ContainerObservation, ObservedRuntimeState, RuntimeInstance};

#[derive(Debug, PartialEq, Eq)]
// Combines persisted operator intent with the latest observed runtime state for status commands.
pub struct RuntimeObservation {
    pub desired_runtime_state: DesiredRuntimeState,
    pub observed_runtime_state: ObservedRuntimeState,
    pub runtime_id: RuntimeInstanceId,
    pub container_id: ContainerId,
    pub observed_endpoint: Option<SocketAddr>,
}

#[derive(Debug)]
pub enum RuntimeLifecycleError {
    NotDeployed {
        application_name: String,
    },
    ContainerMissing {
        application_name: String,
    },
    RuntimeChanged {
        runtime_id: String,
    },
    InvalidDesiredState {
        state: String,
    },
    Store {
        source: RuntimeStoreError,
    },
    ApplicationStore {
        source: ApplicationStoreError,
    },
    Observe {
        runtime_id: String,
        source: ObserveContainerError,
    },
    Control {
        operation: &'static str,
        runtime_id: String,
        source: Box<ControlContainerError>,
    },
    Supervision {
        operation: &'static str,
        runtime_id: String,
        source: QuadletError,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for RuntimeLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotDeployed { application_name } => write!(
                formatter,
                "application `{application_name}` is not deployed"
            ),
            Self::ContainerMissing { application_name } => write!(
                formatter,
                "the container of application `{application_name}` is missing; run `pneuma app start` to recover it or `pneuma app deploy` to recreate it"
            ),
            Self::RuntimeChanged { runtime_id } => write!(
                formatter,
                "runtime `{runtime_id}` changed while it was being controlled"
            ),
            Self::InvalidDesiredState { state } => write!(
                formatter,
                "application has invalid persisted desired state `{state}`"
            ),
            Self::Store { source } => {
                write!(formatter, "failed to control application runtime: {source}")
            }
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to control application runtime: {source}")
            }
            Self::Observe { runtime_id, source } => write!(
                formatter,
                "failed to observe runtime `{runtime_id}`: {source}"
            ),
            Self::Control {
                operation,
                runtime_id,
                source,
            } => write!(
                formatter,
                "failed while {operation} runtime `{runtime_id}`: {source}"
            ),
            Self::Supervision {
                operation,
                runtime_id,
                source,
            } => write!(
                formatter,
                "failed while {operation} supervised runtime `{runtime_id}`: {source}"
            ),
            Self::Persistence { source } => {
                write!(formatter, "failed to control application runtime: {source}")
            }
        }
    }
}

impl Error for RuntimeLifecycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Observe { source, .. } => Some(source),
            Self::Control { source, .. } => Some(source.as_ref()),
            Self::Supervision { source, .. } => Some(source),
            Self::Store { source } => Some(source),
            Self::ApplicationStore { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::NotDeployed { .. }
            | Self::ContainerMissing { .. }
            | Self::RuntimeChanged { .. }
            | Self::InvalidDesiredState { .. } => None,
        }
    }
}

// Observes the current runtime and persists its state without changing the operator's intent.
pub fn report_application_status(
    connection: &mut Connection,
    application_id: &ApplicationId,
    application_name: &str,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    let runtime = load_current_runtime(connection, application_id, application_name)?;
    let desired_runtime_state = load_desired_state(connection, application_id)?;
    let observation =
        observe_container(runtime.external_runtime_id.as_str(), runtime.container_port).map_err(
            |source| RuntimeLifecycleError::Observe {
                runtime_id: runtime.id.to_string(),
                source,
            },
        )?;
    if *observation.state() == ObservedRuntimeState::Missing {
        if desired_runtime_state == DesiredRuntimeState::Stopped {
            persist_observation(connection, &runtime, &observation)?;
            return Ok(RuntimeObservation {
                desired_runtime_state,
                observed_runtime_state: ObservedRuntimeState::Missing,
                runtime_id: runtime.id,
                container_id: runtime.external_runtime_id,
                observed_endpoint: None,
            });
        }
        persist_observation(connection, &runtime, &observation)?;
        return Err(RuntimeLifecycleError::ContainerMissing {
            application_name: application_name.to_owned(),
        });
    }
    persist_observation(connection, &runtime, &observation)?;

    Ok(RuntimeObservation {
        desired_runtime_state,
        observed_runtime_state: observation.state().clone(),
        runtime_id: runtime.id,
        container_id: runtime.external_runtime_id,
        observed_endpoint: observation.observed_endpoint(),
    })
}

// Records stopped intent before delegating the runtime transition to the shared controller.
pub fn stop_application(
    connection: &mut Connection,
    application_id: &ApplicationId,
    application_name: &str,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    transition_application(
        connection,
        application_id,
        application_name,
        DesiredRuntimeState::Stopped,
        ObservedRuntimeState::Stopped,
        "stopping",
        stop_container,
    )
}

// Records running intent before delegating the runtime transition to the shared controller.
pub fn start_application(
    connection: &mut Connection,
    application_id: &ApplicationId,
    application_name: &str,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    transition_application(
        connection,
        application_id,
        application_name,
        DesiredRuntimeState::Running,
        ObservedRuntimeState::Running,
        "starting",
        start_container,
    )
}

// Coordinates intent persistence, external control, and observation while preserving a stable
// runtime record across Quadlet container recreation.
fn transition_application(
    connection: &mut Connection,
    application_id: &ApplicationId,
    application_name: &str,
    desired_runtime_state: DesiredRuntimeState,
    target: ObservedRuntimeState,
    operation: &'static str,
    control: fn(&str) -> Result<ContainerCommandOutput, ControlContainerError>,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    let runtime = load_current_runtime(connection, application_id, application_name)?;
    // The desired state is the operator's intent and is persisted before any external
    // effect, so an interrupted control operation still leaves the intent recorded.
    set_desired_state(connection, application_id, desired_runtime_state)?;
    let current = observe_current_runtime(connection, &runtime, application_name)?;
    let observation = current.observation;
    let external_runtime_id = current.container_id;
    if *observation.state() == ObservedRuntimeState::Missing {
        // When the operator wants the application stopped and the container is missing
        // (removed by the Quadlet ExecStop), deduce a stopped observation without marking
        // the runtime as removed so subsequent stop/start/status commands can still find it.
        if desired_runtime_state == DesiredRuntimeState::Stopped {
            persist_observation(connection, &runtime, &observation)?;
            return Ok(RuntimeObservation {
                desired_runtime_state,
                observed_runtime_state: ObservedRuntimeState::Missing,
                runtime_id: runtime.id,
                container_id: external_runtime_id,
                observed_endpoint: None,
            });
        }
        // When the operator wants the application running and the unit exists, attempt to
        // start it and re-observe (Quadlet recreates the container under the stable name).
        if desired_runtime_state == DesiredRuntimeState::Running {
            let unit = unit_name(application_name, runtime.deployment_id.as_str());
            if unit_exists(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                operation: "checking Quadlet unit for",
                runtime_id: runtime.id.to_string(),
                source,
            })? {
                start_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.id.to_string(),
                    source,
                })?;
                let current = observe_current_runtime(connection, &runtime, application_name)?;
                let new_observation = current.observation;
                let new_external_runtime_id = current.container_id;
                if *new_observation.state() != ObservedRuntimeState::Missing {
                    persist_observation(connection, &runtime, &new_observation)?;
                    return Ok(RuntimeObservation {
                        desired_runtime_state,
                        observed_runtime_state: new_observation.state().clone(),
                        runtime_id: runtime.id,
                        container_id: new_external_runtime_id,
                        observed_endpoint: new_observation.observed_endpoint(),
                    });
                }
            }
        }
        persist_observation(connection, &runtime, &observation)?;
        return Err(RuntimeLifecycleError::ContainerMissing {
            application_name: application_name.to_owned(),
        });
    }
    let observation = if *observation.state() == target {
        observation
    } else {
        let unit = unit_name(application_name, runtime.deployment_id.as_str());
        if unit_exists(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
            operation: "checking Quadlet unit for",
            runtime_id: runtime.id.to_string(),
            source,
        })? {
            if desired_runtime_state == DesiredRuntimeState::Running {
                start_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.id.to_string(),
                    source,
                })?;
            } else {
                stop_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.id.to_string(),
                    source,
                })?;
            }
        } else {
            control(external_runtime_id.as_str()).map_err(|source| {
                RuntimeLifecycleError::Control {
                    operation,
                    runtime_id: runtime.id.to_string(),
                    source: Box::new(source),
                }
            })?;
        }
        observe_container(external_runtime_id.as_str(), runtime.container_port).map_err(
            |source| RuntimeLifecycleError::Observe {
                runtime_id: runtime.id.to_string(),
                source,
            },
        )?
    };
    // When stopping via Quadlet, the ExecStop removes the container, so the observation
    // after stop_unit is Missing. Deduce a stopped observation without marking the runtime
    // as removed so subsequent stop/start/status commands can still find it.
    if desired_runtime_state == DesiredRuntimeState::Stopped
        && *observation.state() == ObservedRuntimeState::Missing
    {
        persist_observation(connection, &runtime, &observation)?;
        return Ok(RuntimeObservation {
            desired_runtime_state,
            observed_runtime_state: ObservedRuntimeState::Missing,
            runtime_id: runtime.id,
            container_id: external_runtime_id,
            observed_endpoint: None,
        });
    }
    persist_observation(connection, &runtime, &observation)?;

    Ok(RuntimeObservation {
        desired_runtime_state,
        observed_runtime_state: observation.state().clone(),
        runtime_id: runtime.id,
        container_id: external_runtime_id,
        observed_endpoint: observation.observed_endpoint(),
    })
}

// Quadlet recreates the container under the stable `pneuma-{application}-{deployment}`
// name with a fresh id whenever its unit restarts (for example, after a reboot). The
// persisted runtime identity can therefore go stale; reconcile it against the name
// before concluding the runtime is gone.
struct CurrentRuntimeObservation {
    observation: ContainerObservation,
    container_id: ContainerId,
}

fn observe_current_runtime(
    connection: &Connection,
    runtime: &RuntimeInstance,
    application_name: &str,
) -> Result<CurrentRuntimeObservation, RuntimeLifecycleError> {
    let observation =
        observe_container(runtime.external_runtime_id.as_str(), runtime.container_port).map_err(
            |source| RuntimeLifecycleError::Observe {
                runtime_id: runtime.id.to_string(),
                source,
            },
        )?;
    if *observation.state() != ObservedRuntimeState::Missing {
        return Ok(CurrentRuntimeObservation {
            observation,
            container_id: runtime.external_runtime_id.clone(),
        });
    }
    let resolved = match resolve_container_id(&container_name(
        application_name,
        runtime.deployment_id.as_str(),
    )) {
        Ok(id) => id,
        Err(_) => {
            return Ok(CurrentRuntimeObservation {
                observation,
                container_id: runtime.external_runtime_id.clone(),
            });
        }
    };
    let reconciled = runtime_store::reconcile_external_runtime_id(
        connection,
        runtime.id.as_str(),
        runtime.external_runtime_id.as_str(),
        &resolved,
    )
    .map_err(|source| RuntimeLifecycleError::Store { source })?;
    if reconciled == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: runtime.id.to_string(),
        });
    }
    let observation = observe_container(&resolved, runtime.container_port).map_err(|source| {
        RuntimeLifecycleError::Observe {
            runtime_id: runtime.id.to_string(),
            source,
        }
    })?;
    Ok(CurrentRuntimeObservation {
        observation,
        container_id: ContainerId::from(resolved),
    })
}

// Loads the active successful runtime, rejecting lifecycle commands for undeployed applications.
fn load_current_runtime(
    connection: &Connection,
    application_id: &ApplicationId,
    application_name: &str,
) -> Result<RuntimeInstance, RuntimeLifecycleError> {
    runtime_store::load_current_successful_runtime(connection, application_id.as_str())
        .map_err(|source| RuntimeLifecycleError::Store { source })?
        .ok_or_else(|| RuntimeLifecycleError::NotDeployed {
            application_name: application_name.to_owned(),
        })
}

// Maps persisted desired state into the domain value and surfaces corrupt values explicitly.
fn load_desired_state(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<DesiredRuntimeState, RuntimeLifecycleError> {
    match application_store::load_desired_runtime_state(connection, application_id.as_str()) {
        Ok(state) => Ok(state),
        Err(ApplicationStoreError::InvalidDesiredRuntimeState { state, .. }) => {
            Err(RuntimeLifecycleError::InvalidDesiredState { state })
        }
        Err(source) => Err(RuntimeLifecycleError::ApplicationStore { source }),
    }
}

// Updates operator intent with compare-and-set semantics so concurrent changes are not lost.
fn set_desired_state(
    connection: &Connection,
    application_id: &ApplicationId,
    desired_runtime_state: DesiredRuntimeState,
) -> Result<(), RuntimeLifecycleError> {
    let expected = load_desired_state(connection, application_id)?;
    let updated = application_store::compare_and_set_desired_runtime_state(
        connection,
        application_id.as_str(),
        expected,
        desired_runtime_state,
    )
    .map_err(|source| RuntimeLifecycleError::ApplicationStore { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: application_id.to_string(),
        });
    }
    Ok(())
}

// Persists an observation only while the runtime record remains current.
fn persist_observation(
    connection: &Connection,
    runtime: &RuntimeInstance,
    observation: &ContainerObservation,
) -> Result<(), RuntimeLifecycleError> {
    let updated = runtime_store::persist_observation(connection, runtime.id.as_str(), observation)
        .map_err(|source| RuntimeLifecycleError::Store { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: runtime.id.to_string(),
        });
    }
    Ok(())
}
