use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

#[derive(Debug)]
pub enum RuntimeStoreError {
    NotFound { runtime_id: String },
    InvalidState { runtime_id: String, state: String },
    PortAlreadyReserved { port: u16 },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for RuntimeStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { runtime_id } => {
                write!(formatter, "runtime `{runtime_id}` not found")
            }
            Self::InvalidState { runtime_id, state } => {
                write!(
                    formatter,
                    "runtime `{runtime_id}` has invalid state `{state}`"
                )
            }
            Self::PortAlreadyReserved { port } => {
                write!(formatter, "port {port} is already reserved")
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
            Self::NotFound { .. }
            | Self::InvalidState { .. }
            | Self::PortAlreadyReserved { .. } => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RuntimeInstance {
    pub id: String,
    pub application_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
    pub state: String,
    pub host_address: String,
    pub host_port: u16,
    pub container_port: u16,
    pub last_observed_state: Option<String>,
    pub created_at: String,
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

pub fn load_runtime_by_external_id(
    connection: &Connection,
    external_runtime_id: &str,
) -> Result<RuntimeInstance, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, deployment_id, external_runtime_id,
                    state, host_address, host_port, container_port,
                    last_observed_state, created_at
             FROM runtime_instances
             WHERE external_runtime_id = ?1",
            [external_runtime_id],
            |row| {
                Ok(RuntimeInstance {
                    id: row.get(0)?,
                    application_id: row.get(1)?,
                    deployment_id: row.get(2)?,
                    external_runtime_id: row.get(3)?,
                    state: row.get(4)?,
                    host_address: row.get(5)?,
                    host_port: row.get(6)?,
                    container_port: row.get(7)?,
                    last_observed_state: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })?
        .ok_or_else(|| RuntimeStoreError::NotFound {
            runtime_id: external_runtime_id.to_owned(),
        })
}

pub fn update_external_runtime_id(
    connection: &Connection,
    runtime_id: &str,
    external_runtime_id: &str,
) -> Result<(), RuntimeStoreError> {
    connection
        .execute(
            "UPDATE runtime_instances
             SET external_runtime_id = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![external_runtime_id, runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

pub fn update_observation_running(
    connection: &Connection,
    runtime_id: &str,
    host_port: u16,
) -> Result<(), RuntimeStoreError> {
    connection
        .execute(
            "UPDATE runtime_instances
             SET last_observed_state = 'running',
                 host_port = ?1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![host_port, runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

pub fn update_observation_stopped(
    connection: &Connection,
    runtime_id: &str,
) -> Result<(), RuntimeStoreError> {
    connection
        .execute(
            "UPDATE runtime_instances
             SET last_observed_state = 'stopped', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            [runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

pub fn mark_missing(connection: &Connection, runtime_id: &str) -> Result<(), RuntimeStoreError> {
    connection
        .execute(
            "UPDATE runtime_instances
             SET last_observed_state = 'missing',
                 removed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            [runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

pub fn stop_previous_runtime(
    transaction: &Transaction<'_>,
    application_id: &str,
    exclude_runtime_id: &str,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'stopped', updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1
               AND state = 'running'
               AND removed_at IS NULL
               AND id != ?2",
            params![application_id, exclude_runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
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

pub fn load_active_runtime_id(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<String>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT ri.id FROM runtime_instances ri
             JOIN applications a ON a.active_deployment_id = ri.deployment_id
             WHERE a.id = ?1 AND ri.state = 'running' AND ri.removed_at IS NULL",
            [application_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

pub fn load_running_runtime_for_deployment(
    connection: &Connection,
    deployment_id: &str,
) -> Result<Option<(String, String, String)>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT id, deployment_id, external_runtime_id
             FROM runtime_instances
             WHERE deployment_id = ?1 AND state = 'running' AND removed_at IS NULL",
            [deployment_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

pub fn remove_runtime(
    transaction: &Transaction<'_>,
    runtime_id: &str,
) -> Result<bool, RuntimeStoreError> {
    let updated = transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'removed', removed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'stopped'",
            [runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn mark_starting_as_missing(
    connection: &Connection,
    runtime_id: &str,
) -> Result<(), RuntimeStoreError> {
    connection
        .execute(
            "UPDATE runtime_instances
             SET last_observed_state = 'missing',
                 removed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'starting'",
            [runtime_id],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
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
