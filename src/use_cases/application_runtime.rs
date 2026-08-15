use std::error::Error;
use std::fmt;
use std::net::SocketAddr;

use rusqlite::Connection;

use crate::adapters::local_runtime::{
    ContainerCommandOutput, ControlContainerError, ObserveContainerError, observe_container,
    resolve_container_id, start_container, stop_container,
};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::adapters::systemd_quadlet::{
    QuadletError, container_name, start as start_unit, stop as stop_unit, unit_exists, unit_name,
};
use crate::domain::runtime::{
    ContainerObservation, DesiredRuntimeState, ObservedRuntimeState, RuntimeInstance,
};

#[derive(Debug, PartialEq, Eq)]
pub struct RuntimeObservation {
    pub desired_runtime_state: DesiredRuntimeState,
    pub observed_runtime_state: ObservedRuntimeState,
    pub runtime_id: String,
    pub container_id: String,
    pub endpoint: Option<SocketAddr>,
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
            Self::Persistence { source } => Some(source),
            Self::NotDeployed { .. }
            | Self::ContainerMissing { .. }
            | Self::RuntimeChanged { .. }
            | Self::InvalidDesiredState { .. } => None,
        }
    }
}

pub fn report_application_status(
    connection: &mut Connection,
    application_id: &str,
    application_name: &str,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    let runtime = load_current_runtime(connection, application_id, application_name)?;
    let desired_runtime_state = load_desired_state(connection, application_id)?;
    let (observation, external_runtime_id) =
        observe_current_runtime(connection, &runtime, application_name)?;
    if observation.state == ObservedRuntimeState::Missing {
        // When the operator wants the application stopped, the Quadlet ExecStop removes
        // the container, so a missing container is the expected stopped state. Report it
        // as stopped without marking the runtime removed so subsequent commands still
        // find it (mirroring transition_application).
        if desired_runtime_state == DesiredRuntimeState::Stopped {
            let stopped_observation = ContainerObservation {
                state: ObservedRuntimeState::Stopped,
                endpoint: None,
            };
            persist_observation(connection, &runtime, &stopped_observation)?;
            return Ok(RuntimeObservation {
                desired_runtime_state,
                observed_runtime_state: ObservedRuntimeState::Stopped,
                runtime_id: runtime.id,
                container_id: external_runtime_id,
                endpoint: None,
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
        observed_runtime_state: observation.state,
        runtime_id: runtime.id,
        container_id: external_runtime_id,
        endpoint: observation.endpoint,
    })
}

pub fn stop_application(
    connection: &mut Connection,
    application_id: &str,
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

pub fn start_application(
    connection: &mut Connection,
    application_id: &str,
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

fn transition_application(
    connection: &mut Connection,
    application_id: &str,
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
    let (observation, external_runtime_id) =
        observe_current_runtime(connection, &runtime, application_name)?;
    if observation.state == ObservedRuntimeState::Missing {
        // When the operator wants the application stopped and the container is missing
        // (removed by the Quadlet ExecStop), deduce a stopped observation without marking
        // the runtime as removed so subsequent stop/start/status commands can still find it.
        if desired_runtime_state == DesiredRuntimeState::Stopped {
            let stopped_observation = ContainerObservation {
                state: ObservedRuntimeState::Stopped,
                endpoint: None,
            };
            persist_observation(connection, &runtime, &stopped_observation)?;
            return Ok(RuntimeObservation {
                desired_runtime_state,
                observed_runtime_state: ObservedRuntimeState::Stopped,
                runtime_id: runtime.id,
                container_id: external_runtime_id,
                endpoint: None,
            });
        }
        // When the operator wants the application running and the unit exists, attempt to
        // start it and re-observe (Quadlet recreates the container under the stable name).
        if desired_runtime_state == DesiredRuntimeState::Running {
            let unit = unit_name(application_name, &runtime.deployment_id);
            if unit_exists(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                operation: "checking Quadlet unit for",
                runtime_id: runtime.id.clone(),
                source,
            })? {
                start_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.id.clone(),
                    source,
                })?;
                let (new_observation, new_external_runtime_id) =
                    observe_current_runtime(connection, &runtime, application_name)?;
                if new_observation.state != ObservedRuntimeState::Missing {
                    persist_observation(connection, &runtime, &new_observation)?;
                    return Ok(RuntimeObservation {
                        desired_runtime_state,
                        observed_runtime_state: new_observation.state,
                        runtime_id: runtime.id,
                        container_id: new_external_runtime_id,
                        endpoint: new_observation.endpoint,
                    });
                }
            }
        }
        persist_observation(connection, &runtime, &observation)?;
        return Err(RuntimeLifecycleError::ContainerMissing {
            application_name: application_name.to_owned(),
        });
    }
    let observation = if observation.state == target {
        observation
    } else {
        let unit = unit_name(application_name, &runtime.deployment_id);
        if unit_exists(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
            operation: "checking Quadlet unit for",
            runtime_id: runtime.id.clone(),
            source,
        })? {
            if desired_runtime_state == DesiredRuntimeState::Running {
                start_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.id.clone(),
                    source,
                })?;
            } else {
                stop_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.id.clone(),
                    source,
                })?;
            }
        } else {
            control(&external_runtime_id).map_err(|source| RuntimeLifecycleError::Control {
                operation,
                runtime_id: runtime.id.clone(),
                source: Box::new(source),
            })?;
        }
        observe_container(&external_runtime_id, runtime.container_port).map_err(|source| {
            RuntimeLifecycleError::Observe {
                runtime_id: runtime.id.clone(),
                source,
            }
        })?
    };
    // When stopping via Quadlet, the ExecStop removes the container, so the observation
    // after stop_unit is Missing. Deduce a stopped observation without marking the runtime
    // as removed so subsequent stop/start/status commands can still find it.
    if desired_runtime_state == DesiredRuntimeState::Stopped
        && observation.state == ObservedRuntimeState::Missing
    {
        let stopped_observation = ContainerObservation {
            state: ObservedRuntimeState::Stopped,
            endpoint: None,
        };
        persist_observation(connection, &runtime, &stopped_observation)?;
        return Ok(RuntimeObservation {
            desired_runtime_state,
            observed_runtime_state: ObservedRuntimeState::Stopped,
            runtime_id: runtime.id,
            container_id: external_runtime_id,
            endpoint: None,
        });
    }
    persist_observation(connection, &runtime, &observation)?;

    Ok(RuntimeObservation {
        desired_runtime_state,
        observed_runtime_state: observation.state,
        runtime_id: runtime.id,
        container_id: external_runtime_id,
        endpoint: observation.endpoint,
    })
}

// Quadlet recreates the container under the stable `pneuma-{application}-{deployment}`
// name with a fresh id whenever its unit restarts (for example, after a reboot). The
// persisted runtime identity can therefore go stale; reconcile it against the name
// before concluding the runtime is gone.
fn observe_current_runtime(
    connection: &Connection,
    runtime: &RuntimeInstance,
    application_name: &str,
) -> Result<(ContainerObservation, String), RuntimeLifecycleError> {
    let observation = observe_container(&runtime.external_runtime_id, runtime.container_port)
        .map_err(|source| RuntimeLifecycleError::Observe {
            runtime_id: runtime.id.clone(),
            source,
        })?;
    if observation.state != ObservedRuntimeState::Missing {
        return Ok((observation, runtime.external_runtime_id.clone()));
    }
    let resolved =
        match resolve_container_id(&container_name(application_name, &runtime.deployment_id)) {
            Ok(id) => id,
            Err(_) => return Ok((observation, runtime.external_runtime_id.clone())),
        };
    let reconciled = runtime_store::reconcile_external_runtime_id(
        connection,
        &runtime.id,
        &runtime.external_runtime_id,
        &resolved,
    )
    .map_err(|source| RuntimeLifecycleError::Store { source })?;
    if !reconciled {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: runtime.id.clone(),
        });
    }
    let observation = observe_container(&resolved, runtime.container_port).map_err(|source| {
        RuntimeLifecycleError::Observe {
            runtime_id: runtime.id.clone(),
            source,
        }
    })?;
    Ok((observation, resolved))
}

fn load_current_runtime(
    connection: &Connection,
    application_id: &str,
    application_name: &str,
) -> Result<RuntimeInstance, RuntimeLifecycleError> {
    runtime_store::load_current_successful_runtime(connection, application_id)
        .map_err(|source| RuntimeLifecycleError::Store { source })?
        .ok_or_else(|| RuntimeLifecycleError::NotDeployed {
            application_name: application_name.to_owned(),
        })
}

fn load_desired_state(
    connection: &Connection,
    application_id: &str,
) -> Result<DesiredRuntimeState, RuntimeLifecycleError> {
    match runtime_store::load_desired_runtime_state(connection, application_id) {
        Ok(state) => Ok(state),
        Err(RuntimeStoreError::InvalidDesiredState { state, .. }) => {
            Err(RuntimeLifecycleError::InvalidDesiredState { state })
        }
        Err(source) => Err(RuntimeLifecycleError::Store { source }),
    }
}

fn set_desired_state(
    connection: &Connection,
    application_id: &str,
    desired_runtime_state: DesiredRuntimeState,
) -> Result<(), RuntimeLifecycleError> {
    let expected = load_desired_state(connection, application_id)?;
    let updated = runtime_store::compare_and_set_desired_runtime_state(
        connection,
        application_id,
        expected,
        desired_runtime_state,
    )
    .map_err(|source| RuntimeLifecycleError::Store { source })?;
    if !updated {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: application_id.to_owned(),
        });
    }
    Ok(())
}

fn persist_observation(
    connection: &Connection,
    runtime: &RuntimeInstance,
    observation: &ContainerObservation,
) -> Result<(), RuntimeLifecycleError> {
    let updated = runtime_store::persist_observation(connection, &runtime.id, observation)
        .map_err(|source| RuntimeLifecycleError::Store { source })?;
    if !updated {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: runtime.id.clone(),
        });
    }
    Ok(())
}
