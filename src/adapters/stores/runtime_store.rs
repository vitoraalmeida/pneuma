use std::error::Error;
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::adapters::stores::PersistenceOutcome;
use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId};
use crate::domain::runtime::{
    ContainerId, ContainerObservation, ContainerPort, ExpectedRuntimeEndpoint,
    ObservedRuntimeState, PreviousRuntime, RuntimeInstance, RuntimeRegistration, RuntimeRetirement,
    RuntimeState,
};

#[derive(Debug)]
pub enum RuntimeStoreError {
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for RuntimeStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
        }
    }
}

// Allocates a runtime ID beside endpoint registration in the same SQLite transaction.
pub fn generate_id(connection: &Connection) -> Result<RuntimeInstanceId, RuntimeStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map(RuntimeInstanceId::from)
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

// Checks whether a non-removed runtime already owns the requested loopback endpoint.
pub fn port_is_reserved(
    connection: &Connection,
    endpoint: &ExpectedRuntimeEndpoint,
) -> Result<bool, RuntimeStoreError> {
    let socket_addr = endpoint.socket_addr();
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_instances
             WHERE host_address = ?1 AND host_port = ?2 AND removed_at IS NULL)",
            params![socket_addr.ip().to_string(), socket_addr.port()],
            |row| row.get(0),
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

// Persists a candidate runtime and its reserved loopback endpoint after external creation.
pub fn insert_runtime(
    transaction: &Transaction<'_>,
    registration: &RuntimeRegistration,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "INSERT INTO runtime_instances (
                id, application_id, deployment_id, external_runtime_id,
                state, host_address, host_port, container_port,
                last_observed_state, last_observed_at, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'running', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                registration.id.as_str(),
                registration.application_id.as_str(),
                registration.deployment_id.as_str(),
                registration.external_runtime_id.as_str(),
                runtime_state_value(RuntimeState::Starting),
                registration.expected_endpoint.socket_addr().ip().to_string(),
                registration.expected_endpoint.socket_addr().port(),
                registration.container_port.get()
            ],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

// Loads the non-removed runtime belonging to the active successful Deployment.
pub fn load_current_successful_runtime(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Option<RuntimeInstance>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT
                runtime_instances.id,
                runtime_instances.application_id,
                runtime_instances.deployment_id,
                runtime_instances.external_runtime_id,
                runtime_instances.state,
                runtime_instances.host_address,
                runtime_instances.host_port,
                runtime_instances.container_port,
                runtime_instances.last_observed_state,
                runtime_instances.last_observed_at,
                runtime_instances.exit_code,
                runtime_instances.observation_reason,
                runtime_instances.removed_at
             FROM runtime_instances
             JOIN applications
                ON applications.active_deployment_id = runtime_instances.deployment_id
             JOIN deployments ON deployments.id = runtime_instances.deployment_id
             WHERE applications.id = ?1
               AND runtime_instances.state IN ('running', 'stopped')
               AND runtime_instances.removed_at IS NULL
               AND deployments.status = 'succeeded'",
            [application_id.as_str()],
            map_runtime_instance,
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

// Replaces an external container ID only when the logical runtime still has the expected ID.
pub fn reconcile_external_runtime_id(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
    expected_external_runtime_id: &ContainerId,
    replacement_external_runtime_id: &ContainerId,
) -> Result<PersistenceOutcome, RuntimeStoreError> {
    let updated = connection
        .execute(
            "UPDATE runtime_instances
              SET external_runtime_id = ?1,
                  last_observed_state = 'running',
                  last_observed_at = CURRENT_TIMESTAMP,
                  updated_at = CURRENT_TIMESTAMP
              WHERE id = ?2
                AND external_runtime_id = ?3
                AND removed_at IS NULL",
            params![
                replacement_external_runtime_id.as_str(),
                runtime_id.as_str(),
                expected_external_runtime_id.as_str()
            ],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Records an observed runtime state without reviving a runtime that has been retired.
pub fn persist_observation(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
    observation: &ContainerObservation,
) -> Result<PersistenceOutcome, RuntimeStoreError> {
    let state = observed_runtime_state_value(observation.state());
    let updated = connection
        .execute(
            "UPDATE runtime_instances
         SET last_observed_state = ?2,
              last_observed_at = CURRENT_TIMESTAMP,
              updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1 AND removed_at IS NULL",
            params![runtime_id.as_str(), state],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Advances a non-removed candidate from starting to running exactly once.
pub fn start_runtime(
    transaction: &Transaction<'_>,
    runtime_id: &RuntimeInstanceId,
) -> Result<PersistenceOutcome, RuntimeStoreError> {
    let updated = transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'running', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'starting' AND removed_at IS NULL",
            [runtime_id.as_str()],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Stops prior live runtimes during promotion; no matching runtime is a normal outcome.
pub fn stop_other_running_runtimes(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    candidate_runtime_id: &RuntimeInstanceId,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'stopped', updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1
               AND state = 'running'
               AND removed_at IS NULL
               AND id != ?2",
            params![application_id.as_str(), candidate_runtime_id.as_str()],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(())
}

// Reads the logical lifecycle state for cleanup and transition decisions.
pub fn load_runtime_state(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<Option<RuntimeState>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT state FROM runtime_instances WHERE id = ?1",
            [runtime_id.as_str()],
            |row| {
                let value: String = row.get(0)?;
                runtime_state_from_value(&value)
                    .ok_or_else(|| invalid_text_value(0, "runtime state", &value))
            },
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

// Finds another live runtime that may need retirement after candidate promotion.
pub fn load_previous_runtime(
    connection: &Connection,
    application_id: &ApplicationId,
    candidate_runtime_id: &RuntimeInstanceId,
) -> Result<Option<PreviousRuntime>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT id, deployment_id, external_runtime_id
             FROM runtime_instances
             WHERE application_id = ?1
               AND state = 'running'
               AND removed_at IS NULL
               AND id != ?2",
            [application_id.as_str(), candidate_runtime_id.as_str()],
            |row| {
                Ok(PreviousRuntime {
                    runtime_id: RuntimeInstanceId::from(row.get::<_, String>(0)?),
                    deployment_id: DeploymentId::from(row.get::<_, String>(1)?),
                    external_runtime_id: hydrate_container_id(2, &row.get::<_, String>(2)?)?,
                })
            },
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

// Resolves a logical runtime from the container identity observed from Podman.
pub fn load_runtime_by_external_id(
    connection: &Connection,
    external_runtime_id: &ContainerId,
) -> Result<Option<RuntimeInstance>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, deployment_id, external_runtime_id,
                    state, host_address, host_port, container_port,
                    last_observed_state, last_observed_at, exit_code,
                    observation_reason, removed_at
             FROM runtime_instances WHERE external_runtime_id = ?1",
            [external_runtime_id.as_str()],
            map_runtime_instance,
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

// Finds the candidate runtime registered by one deployment without confusing it with an active runtime.
pub fn load_runtime_by_deployment(
    connection: &Connection,
    deployment_id: &DeploymentId,
) -> Result<Option<RuntimeInstance>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT id, application_id, deployment_id, external_runtime_id,
                    state, host_address, host_port, container_port,
                    last_observed_state, last_observed_at, exit_code,
                    observation_reason, removed_at
             FROM runtime_instances WHERE deployment_id = ?1 AND removed_at IS NULL",
            [deployment_id.as_str()],
            map_runtime_instance,
        )
        .optional()
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

// Tombstones only a stopped runtime, preserving lifecycle transition ordering.
pub fn mark_runtime_removed(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PersistenceOutcome, RuntimeStoreError> {
    let updated = connection
        .execute(
            "UPDATE runtime_instances
              SET state = 'stopped', removed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'stopped' AND removed_at IS NULL",
            [runtime_id.as_str()],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Records failed candidate creation as missing and explicitly retired while it is still starting.
pub fn mark_starting_runtime_missing(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PersistenceOutcome, RuntimeStoreError> {
    let updated = connection
        .execute(
            "UPDATE runtime_instances
              SET state = 'stopped',
                  last_observed_state = 'missing',
                 last_observed_at = CURRENT_TIMESTAMP,
                 removed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'starting' AND removed_at IS NULL",
            [runtime_id.as_str()],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Resolves the live runtime tied to the Application's active Deployment within a transaction.
pub fn load_active_runtime_for_application(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
) -> Result<Option<RuntimeInstanceId>, RuntimeStoreError> {
    transaction
        .query_row(
            "SELECT ri.id FROM runtime_instances ri
             JOIN applications a ON a.active_deployment_id = ri.deployment_id
             WHERE ri.application_id = ?1
               AND ri.state = 'running'
               AND ri.removed_at IS NULL",
            [application_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map(|runtime_id| runtime_id.map(RuntimeInstanceId::from))
        .map_err(|source| RuntimeStoreError::Persistence { source })
}

// Advances a running logical runtime to stopped without marking it retired.
pub fn stop_runtime(
    transaction: &Transaction<'_>,
    runtime_id: &RuntimeInstanceId,
) -> Result<PersistenceOutcome, RuntimeStoreError> {
    let updated = transaction
        .execute(
            "UPDATE runtime_instances
             SET state = 'stopped', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'running'",
            [runtime_id.as_str()],
        )
        .map_err(|source| RuntimeStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Maps persisted runtime identity and enforces the loopback-only endpoint invariant.
fn map_runtime_instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeInstance> {
    let state_text = row.get::<_, String>(4)?;
    let removed_at = row.get::<_, Option<String>>(12)?;
    let (state, retirement) = match (state_text.as_str(), removed_at) {
        ("removed", Some(removed_at)) => (
            RuntimeState::Stopped,
            Some(RuntimeRetirement { removed_at }),
        ),
        ("removed", None) => {
            return Err(invalid_text_value(
                4,
                "retired runtime without removed_at",
                &state_text,
            ));
        }
        (state_text, Some(removed_at)) => {
            return Err(invalid_text_value(
                12,
                "active runtime with removed_at",
                &format!("{state_text} ({removed_at})"),
            ));
        }
        (state_text, None) => (
            runtime_state_from_value(state_text)
                .ok_or_else(|| invalid_text_value(4, "runtime state", state_text))?,
            None,
        ),
    };
    let host_address = row.get::<_, String>(5)?;
    if host_address != Ipv4Addr::LOCALHOST.to_string() {
        return Err(invalid_text_value(5, "runtime host address", &host_address));
    }
    let observed_state_text = row.get::<_, String>(8)?;

    Ok(RuntimeInstance {
        id: RuntimeInstanceId::from(row.get::<_, String>(0)?),
        application_id: ApplicationId::from(row.get::<_, String>(1)?),
        deployment_id: DeploymentId::from(row.get::<_, String>(2)?),
        external_runtime_id: hydrate_container_id(3, &row.get::<_, String>(3)?)?,
        state,
        expected_endpoint: ExpectedRuntimeEndpoint::new(SocketAddr::from((
            Ipv4Addr::LOCALHOST,
            row.get::<_, u16>(6)?,
        )))
        .map_err(|error| invalid_text_value(6, "runtime endpoint", &error.to_string()))?,
        container_port: ContainerPort::new(row.get::<_, u16>(7)?)
            .map_err(|error| invalid_text_value(7, "runtime container port", &error.to_string()))?,
        observed_state: observed_runtime_state_from_value(&observed_state_text),
        observed_at: row.get(9)?,
        exit_code: row.get(10)?,
        observation_reason: row.get(11)?,
        retirement,
    })
}

// Converts an invalid persisted text value into a row-mapping error with column context.
fn invalid_text_value(column: usize, field: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {field}: {value}"),
        )),
    )
}

// Hydrates a persisted container identity only when it satisfies the domain invariant.
fn hydrate_container_id(column: usize, value: &str) -> rusqlite::Result<ContainerId> {
    if !ContainerId::is_valid(value) {
        return Err(invalid_text_value(column, "external runtime id", value));
    }
    Ok(ContainerId::from(value.to_owned()))
}
fn outcome(updated: usize) -> PersistenceOutcome {
    if updated == 1 {
        PersistenceOutcome::Updated
    } else {
        PersistenceOutcome::Stale
    }
}
fn runtime_state_value(value: RuntimeState) -> &'static str {
    match value {
        RuntimeState::Starting => "starting",
        RuntimeState::Running => "running",
        RuntimeState::Stopped => "stopped",
        RuntimeState::Failed => "failed",
    }
}
fn runtime_state_from_value(value: &str) -> Option<RuntimeState> {
    match value {
        "starting" => Some(RuntimeState::Starting),
        "running" => Some(RuntimeState::Running),
        "stopped" => Some(RuntimeState::Stopped),
        "failed" => Some(RuntimeState::Failed),
        _ => None,
    }
}
fn observed_runtime_state_value(value: &ObservedRuntimeState) -> &'static str {
    match value {
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
fn observed_runtime_state_from_value(value: &str) -> ObservedRuntimeState {
    match value {
        "missing" => ObservedRuntimeState::Missing,
        "created" => ObservedRuntimeState::Created,
        "starting" => ObservedRuntimeState::Starting,
        "running" => ObservedRuntimeState::Running,
        "stopping" => ObservedRuntimeState::Stopping,
        "stopped" => ObservedRuntimeState::Stopped,
        "failed" => ObservedRuntimeState::Failed,
        status => ObservedRuntimeState::Unknown {
            status: status.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_a_typed_runtime_state_and_rejects_invalid_persisted_text() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_instances (id TEXT PRIMARY KEY, state TEXT NOT NULL);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_instances (id, state) VALUES ('starting', 'starting')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_instances (id, state) VALUES ('invalid', 'invalid')",
                [],
            )
            .unwrap();

        assert_eq!(
            load_runtime_state(&connection, &RuntimeInstanceId::from("starting")).unwrap(),
            Some(RuntimeState::Starting)
        );
        assert!(matches!(
            load_runtime_state(&connection, &RuntimeInstanceId::from("invalid")),
            Err(RuntimeStoreError::Persistence { .. })
        ));
    }

    #[test]
    fn rejects_a_corrupt_persisted_external_runtime_id_instead_of_hydrating_it() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_instances (
                    id TEXT PRIMARY KEY,
                    application_id TEXT NOT NULL,
                    deployment_id TEXT NOT NULL,
                    external_runtime_id TEXT NOT NULL,
                    state TEXT NOT NULL,
                    host_address TEXT NOT NULL,
                    host_port INTEGER NOT NULL,
                    container_port INTEGER NOT NULL,
                    last_observed_state TEXT NOT NULL,
                    last_observed_at TEXT NOT NULL,
                    exit_code INTEGER,
                    observation_reason TEXT,
                    removed_at TEXT
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_instances VALUES
                 ('runtime', 'application', 'deployment', 'not a container id',
                  'running', '127.0.0.1', 30000, 8080, 'running', 'now', NULL, NULL, NULL)",
                [],
            )
            .unwrap();

        let error =
            load_runtime_by_external_id(&connection, &ContainerId::from("not a container id"))
                .expect_err("corrupt persisted identity must not hydrate");
        assert!(error.to_string().contains("external runtime id"));
    }
}
