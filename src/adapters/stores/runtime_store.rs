use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::adapters::local_runtime::{ContainerObservation, ObservedRuntimeState};
use crate::domain::runtime::DesiredRuntimeState;

#[derive(Debug)]
pub enum RuntimeStoreError {
    InvalidDesiredState {
        application_id: String,
        state: String,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for RuntimeStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDesiredState {
                application_id,
                state,
            } => {
                write!(
                    formatter,
                    "application `{application_id}` has invalid desired runtime state `{state}`"
                )
            }
            Self::Persistence { source } => {
                write!(formatter, "runtime store error: {source}")
            }
        }
    }
}

impl Error for RuntimeStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::InvalidDesiredState { .. } => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct CurrentSuccessfulRuntime {
    pub runtime_id: String,
    pub external_runtime_id: String,
    pub deployment_id: String,
    pub container_port: u16,
}

pub fn generate_id(connection: &Connection) -> Result<String, RuntimeStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

pub fn port_is_reserved(
    connection: &Connection,
    host_address: &str,
    host_port: u16,
) -> Result<bool, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_instances
             WHERE host_address = ?1 AND host_port = ?2 AND removed_at IS NULL)",
            params![host_address, host_port],
            |row| row.get(0),
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

#[allow(clippy::too_many_arguments)]
pub fn insert_runtime(
    transaction: &Transaction<'_>,
    id: &str,
    application_id: &str,
    deployment_id: &str,
    external_runtime_id: &str,
    state: &str,
    host_address: &str,
    host_port: u16,
    container_port: u16,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "INSERT INTO runtime_instances (
                id, application_id, deployment_id, external_runtime_id,
                state, host_address, host_port, container_port,
                last_observed_state, last_observed_at, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'running', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                id,
                application_id,
                deployment_id,
                external_runtime_id,
                state,
                host_address,
                host_port,
                container_port
            ],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

pub fn load_current_successful_runtime(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<CurrentSuccessfulRuntime>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT
                runtime_instances.id,
                runtime_instances.external_runtime_id,
                runtime_instances.deployment_id,
                runtime_instances.container_port
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
                Ok(CurrentSuccessfulRuntime {
                    runtime_id: row.get(0)?,
                    external_runtime_id: row.get(1)?,
                    deployment_id: row.get(2)?,
                    container_port: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

pub fn reconcile_external_runtime_id(
    connection: &Connection,
    runtime_id: &str,
    expected_external_runtime_id: &str,
    replacement_external_runtime_id: &str,
) -> Result<bool, RuntimeStoreError> {
    let updated = connection
        .execute(
            "UPDATE runtime_instances
             SET external_runtime_id = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2
               AND external_runtime_id = ?3
               AND removed_at IS NULL",
            params![
                replacement_external_runtime_id,
                runtime_id,
                expected_external_runtime_id
            ],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn persist_observation(
    connection: &Connection,
    runtime_id: &str,
    observation: &ContainerObservation,
) -> Result<bool, RuntimeStoreError> {
    let state = observed_state_database_value(&observation.state);
    let updated = if let Some(endpoint) = observation.endpoint {
        connection.execute(
            "UPDATE runtime_instances
             SET last_observed_state = ?2,
                 host_port = ?3,
                 last_observed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND removed_at IS NULL",
            params![runtime_id, state, endpoint.port()],
        )
    } else {
        connection.execute(
            "UPDATE runtime_instances
             SET last_observed_state = ?2,
                 last_observed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND removed_at IS NULL",
            params![runtime_id, state],
        )
    }
    .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn load_desired_runtime_state(
    connection: &Connection,
    application_id: &str,
) -> Result<DesiredRuntimeState, RuntimeStoreError> {
    let value = connection
        .query_row(
            "SELECT desired_runtime_state FROM applications WHERE id = ?1",
            [application_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    DesiredRuntimeState::from_database(&value).ok_or_else(|| {
        RuntimeStoreError::InvalidDesiredState {
            application_id: application_id.to_owned(),
            state: value,
        }
    })
}

pub fn compare_and_set_desired_runtime_state(
    connection: &Connection,
    application_id: &str,
    expected: DesiredRuntimeState,
    desired: DesiredRuntimeState,
) -> Result<bool, RuntimeStoreError> {
    let updated = connection
        .execute(
            "UPDATE applications
             SET desired_runtime_state = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2 AND desired_runtime_state = ?3",
            params![
                desired.database_value(),
                application_id,
                expected.database_value()
            ],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(updated == 1)
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

pub fn start_runtime(
    transaction: &Transaction<'_>,
    runtime_id: &str,
) -> Result<bool, RuntimeStoreError> {
    let updated = transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'running', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'starting' AND removed_at IS NULL",
            [runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn load_runtime_state(
    connection: &Connection,
    runtime_id: &str,
) -> Result<Option<String>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT state FROM runtime_instances WHERE id = ?1",
            [runtime_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

pub fn load_previous_runtime(
    connection: &Connection,
    application_id: &str,
    candidate_runtime_id: &str,
) -> Result<Option<(String, String, String)>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT id, deployment_id, external_runtime_id
             FROM runtime_instances
             WHERE application_id = ?1
               AND state = 'running'
               AND removed_at IS NULL
               AND id != ?2",
            [application_id, candidate_runtime_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

pub fn mark_runtime_removed(
    connection: &Connection,
    runtime_id: &str,
) -> Result<(), RuntimeStoreError> {
    connection
        .execute(
            "UPDATE runtime_instances
             SET state = 'removed', removed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'stopped' AND removed_at IS NULL",
            [runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

pub fn mark_starting_runtime_missing(
    connection: &Connection,
    runtime_id: &str,
) -> Result<(), RuntimeStoreError> {
    connection
        .execute(
            "UPDATE runtime_instances
             SET last_observed_state = 'missing',
                 last_observed_at = CURRENT_TIMESTAMP,
                 removed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'starting' AND removed_at IS NULL",
            [runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

pub fn load_active_runtime_for_application(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<Option<String>, RuntimeStoreError> {
    transaction
        .query_row(
            "SELECT ri.id FROM runtime_instances ri
             JOIN applications a ON a.active_deployment_id = ri.deployment_id
             WHERE ri.application_id = ?1
               AND ri.state = 'running'
               AND ri.removed_at IS NULL",
            [application_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

pub fn stop_runtime(
    transaction: &Transaction<'_>,
    runtime_id: &str,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'stopped', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'running'",
            [runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}
