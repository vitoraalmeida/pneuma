use std::error::Error;
use std::fmt;
use std::net::SocketAddr;

use rusqlite::{Connection, OptionalExtension, params};

use crate::adapters::local_runtime::{
    ContainerCommandOutput, ContainerObservation, ControlContainerError, ObserveContainerError,
    ObservedRuntimeState, observe_container, resolve_container_id, start_container, stop_container,
};
use crate::adapters::systemd_quadlet::{
    QuadletError, container_name, start as start_unit, stop as stop_unit, unit_exists, unit_name,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesiredRuntimeState {
    Running,
    Stopped,
}

impl DesiredRuntimeState {
    fn from_database(value: &str) -> Option<Self> {
        match value {
            "running" => Some(Self::Running),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }

    fn to_database_value(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
        }
    }
}

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
                "the container of application `{application_name}` is missing; run `pneuma app deploy` to recreate it"
            ),
            Self::RuntimeChanged { runtime_id } => write!(
                formatter,
                "runtime `{runtime_id}` changed while it was being controlled"
            ),
            Self::InvalidDesiredState { state } => write!(
                formatter,
                "application has invalid persisted desired state `{state}`"
            ),
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
            persist_stopped_without_removal(connection, &runtime, &stopped_observation)?;
            return Ok(RuntimeObservation {
                desired_runtime_state,
                observed_runtime_state: ObservedRuntimeState::Stopped,
                runtime_id: runtime.runtime_id,
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
        runtime_id: runtime.runtime_id,
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
            persist_stopped_without_removal(connection, &runtime, &stopped_observation)?;
            return Ok(RuntimeObservation {
                desired_runtime_state,
                observed_runtime_state: ObservedRuntimeState::Stopped,
                runtime_id: runtime.runtime_id,
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
                runtime_id: runtime.runtime_id.clone(),
                source,
            })? {
                start_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.runtime_id.clone(),
                    source,
                })?;
                let (new_observation, new_external_runtime_id) =
                    observe_current_runtime(connection, &runtime, application_name)?;
                if new_observation.state != ObservedRuntimeState::Missing {
                    persist_observation(connection, &runtime, &new_observation)?;
                    return Ok(RuntimeObservation {
                        desired_runtime_state,
                        observed_runtime_state: new_observation.state,
                        runtime_id: runtime.runtime_id,
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
            runtime_id: runtime.runtime_id.clone(),
            source,
        })? {
            if desired_runtime_state == DesiredRuntimeState::Running {
                start_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.runtime_id.clone(),
                    source,
                })?;
            } else {
                stop_unit(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
                    operation,
                    runtime_id: runtime.runtime_id.clone(),
                    source,
                })?;
            }
        } else {
            control(&external_runtime_id).map_err(|source| RuntimeLifecycleError::Control {
                operation,
                runtime_id: runtime.runtime_id.clone(),
                source: Box::new(source),
            })?;
        }
        observe_container(&external_runtime_id, runtime.container_port).map_err(|source| {
            RuntimeLifecycleError::Observe {
                runtime_id: runtime.runtime_id.clone(),
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
        persist_stopped_without_removal(connection, &runtime, &stopped_observation)?;
        return Ok(RuntimeObservation {
            desired_runtime_state,
            observed_runtime_state: ObservedRuntimeState::Stopped,
            runtime_id: runtime.runtime_id,
            container_id: external_runtime_id,
            endpoint: None,
        });
    }
    persist_observation(connection, &runtime, &observation)?;

    Ok(RuntimeObservation {
        desired_runtime_state,
        observed_runtime_state: observation.state,
        runtime_id: runtime.runtime_id,
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
    runtime: &CurrentRuntime,
    application_name: &str,
) -> Result<(ContainerObservation, String), RuntimeLifecycleError> {
    let observation = observe_container(&runtime.external_runtime_id, runtime.container_port)
        .map_err(|source| RuntimeLifecycleError::Observe {
            runtime_id: runtime.runtime_id.clone(),
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
    connection
        .execute(
            "UPDATE runtime_instances
             SET external_runtime_id = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2 AND removed_at IS NULL",
            params![&resolved, &runtime.runtime_id],
        )
        .map_err(|source| RuntimeLifecycleError::Persistence { source })?;
    let observation = observe_container(&resolved, runtime.container_port).map_err(|source| {
        RuntimeLifecycleError::Observe {
            runtime_id: runtime.runtime_id.clone(),
            source,
        }
    })?;
    Ok((observation, resolved))
}

struct CurrentRuntime {
    runtime_id: String,
    external_runtime_id: String,
    container_port: u16,
    deployment_id: String,
}

fn load_current_runtime(
    connection: &Connection,
    application_id: &str,
    application_name: &str,
) -> Result<CurrentRuntime, RuntimeLifecycleError> {
    connection
        .query_row(
            "SELECT
                runtime_instances.id,
                runtime_instances.external_runtime_id,
                runtime_instances.container_port,
                runtime_instances.deployment_id
             FROM runtime_instances
              JOIN applications
                ON applications.active_deployment_id = runtime_instances.deployment_id
              JOIN deployments ON deployments.id = runtime_instances.deployment_id
              WHERE applications.id = ?1
                AND runtime_instances.state IN ('running', 'stopped')
                AND runtime_instances.removed_at IS NULL
                AND deployments.status = 'succeeded'",
            [application_id],
            |row| {
                Ok(CurrentRuntime {
                    runtime_id: row.get(0)?,
                    external_runtime_id: row.get(1)?,
                    container_port: row.get(2)?,
                    deployment_id: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|source| RuntimeLifecycleError::Persistence { source })?
        .ok_or_else(|| RuntimeLifecycleError::NotDeployed {
            application_name: application_name.to_owned(),
        })
}

fn load_desired_state(
    connection: &Connection,
    application_id: &str,
) -> Result<DesiredRuntimeState, RuntimeLifecycleError> {
    let value = connection
        .query_row(
            "SELECT desired_runtime_state FROM applications WHERE id = ?1",
            [application_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|source| RuntimeLifecycleError::Persistence { source })?;
    DesiredRuntimeState::from_database(&value)
        .ok_or_else(|| RuntimeLifecycleError::InvalidDesiredState { state: value })
}

fn set_desired_state(
    connection: &Connection,
    application_id: &str,
    desired_runtime_state: DesiredRuntimeState,
) -> Result<(), RuntimeLifecycleError> {
    connection
        .execute(
            "UPDATE applications
             SET desired_runtime_state = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![desired_runtime_state.to_database_value(), application_id],
        )
        .map_err(|source| RuntimeLifecycleError::Persistence { source })?;
    Ok(())
}

fn persist_observation(
    connection: &Connection,
    runtime: &CurrentRuntime,
    observation: &crate::adapters::local_runtime::ContainerObservation,
) -> Result<(), RuntimeLifecycleError> {
    let state = observed_state_database_value(&observation.state);
    let updated = if observation.state == ObservedRuntimeState::Missing {
        connection.execute(
            "UPDATE runtime_instances
             SET last_observed_state = 'missing',
                 last_observed_at = CURRENT_TIMESTAMP,
                 removed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND removed_at IS NULL",
            [&runtime.runtime_id],
        )
    } else if let Some(endpoint) = observation.endpoint {
        connection.execute(
            "UPDATE runtime_instances
             SET last_observed_state = ?2,
                 host_port = ?3,
                 last_observed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND removed_at IS NULL",
            params![&runtime.runtime_id, state, endpoint.port()],
        )
    } else {
        connection.execute(
            "UPDATE runtime_instances
             SET last_observed_state = ?2,
                 last_observed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND removed_at IS NULL",
            params![&runtime.runtime_id, state],
        )
    }
    .map_err(|source| RuntimeLifecycleError::Persistence { source })?;
    if updated != 1 {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: runtime.runtime_id.clone(),
        });
    }
    Ok(())
}

fn persist_stopped_without_removal(
    connection: &Connection,
    runtime: &CurrentRuntime,
    observation: &crate::adapters::local_runtime::ContainerObservation,
) -> Result<(), RuntimeLifecycleError> {
    let state = observed_state_database_value(&observation.state);
    let updated = connection
        .execute(
            "UPDATE runtime_instances
             SET last_observed_state = ?2,
                 last_observed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND removed_at IS NULL",
            params![&runtime.runtime_id, state],
        )
        .map_err(|source| RuntimeLifecycleError::Persistence { source })?;
    if updated != 1 {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: runtime.runtime_id.clone(),
        });
    }
    Ok(())
}

fn observed_state_database_value(state: &ObservedRuntimeState) -> &'static str {
    match state {
        ObservedRuntimeState::Missing => "missing",
        ObservedRuntimeState::Created => "created",
        ObservedRuntimeState::Starting => "starting",
        ObservedRuntimeState::Running => "running",
        ObservedRuntimeState::Stopping => "stopping",
        ObservedRuntimeState::Stopped => "stopped",
        ObservedRuntimeState::Failed => "failed",
        ObservedRuntimeState::Unknown { .. } => "unknown",
    }
}
