use std::net::{Ipv4Addr, SocketAddr};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::persistence::{
    entity_id, invalid_text_value, observed_runtime_state_from_value, observed_runtime_state_value,
    outcome, runtime_state_from_value, runtime_state_value,
};
use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId};
use crate::domain::runtime::{
    ContainerId, ContainerObservation, ContainerPort, ExpectedRuntimeEndpoint, PreviousRuntime,
    RuntimeInstance, RuntimeRegistration, RuntimeRetirement, RuntimeState,
};

// Allocates a runtime ID beside endpoint registration in the same SQLite transaction.
pub(crate) fn generate_id(connection: &Connection) -> Result<RuntimeInstanceId, rusqlite::Error> {
    let value = connection.query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
        row.get::<_, String>(0)
    })?;
    entity_id(0, &value)
}

// Checks whether a non-removed runtime already owns the requested loopback endpoint.
pub(crate) fn port_is_reserved(
    connection: &Connection,
    endpoint: &ExpectedRuntimeEndpoint,
) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_instances
             WHERE host_port = ?1 AND removed_at IS NULL)",
        [endpoint.socket_addr().port()],
        |row| row.get(0),
    )
}

// Persists a candidate runtime and its reserved loopback endpoint after external creation.
pub(crate) fn insert_runtime(
    transaction: &Transaction<'_>,
    registration: &RuntimeRegistration,
) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "INSERT INTO runtime_instances (
                id, application_id, deployment_id, external_runtime_id,
                state, host_port, container_port,
                last_observed_state, last_observed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', CURRENT_TIMESTAMP)",
        params![
            registration.id.as_str(),
            registration.application_id.as_str(),
            registration.deployment_id.as_str(),
            registration.external_runtime_id.as_str(),
            runtime_state_value(RuntimeState::Starting),
            registration.expected_endpoint.socket_addr().port(),
            registration.container_port.get()
        ],
    )?;
    Ok(())
}

// Loads the non-removed runtime belonging to the active successful Deployment.
pub(crate) fn load_active_successful_runtime(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Option<RuntimeInstance>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT
                runtime_instances.id,
                runtime_instances.application_id,
                runtime_instances.deployment_id,
                runtime_instances.external_runtime_id,
                runtime_instances.state,
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
}

// Replaces an external container ID only when the logical runtime still has the expected ID.
pub(crate) fn reconcile_external_runtime_id(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
    expected_external_runtime_id: &ContainerId,
    replacement_external_runtime_id: &ContainerId,
) -> Result<PersistenceOutcome, rusqlite::Error> {
    let updated = connection.execute(
        "UPDATE runtime_instances
              SET external_runtime_id = ?1,
                  last_observed_state = 'running',
                  last_observed_at = CURRENT_TIMESTAMP
              WHERE id = ?2
                AND external_runtime_id = ?3
                AND removed_at IS NULL",
        params![
            replacement_external_runtime_id.as_str(),
            runtime_id.as_str(),
            expected_external_runtime_id.as_str()
        ],
    )?;
    Ok(outcome(updated))
}

// Records an observed runtime state while the Application lock serializes the
// lifecycle/status workflow that loaded this live runtime.
pub(crate) fn persist_observation(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
    observation: &ContainerObservation,
) -> Result<(), rusqlite::Error> {
    let state = observed_runtime_state_value(observation.state());
    connection.execute(
        "UPDATE runtime_instances
         SET last_observed_state = ?2,
              last_observed_at = CURRENT_TIMESTAMP
          WHERE id = ?1 AND removed_at IS NULL",
        params![runtime_id.as_str(), state],
    )?;
    Ok(())
}

// Advances a non-removed candidate from starting to running exactly once.
pub(crate) fn start_runtime(
    transaction: &Transaction<'_>,
    runtime_id: &RuntimeInstanceId,
) -> Result<PersistenceOutcome, rusqlite::Error> {
    let updated = transaction.execute(
        "UPDATE runtime_instances
              SET state = 'running'
              WHERE id = ?1 AND state = 'starting' AND removed_at IS NULL",
        [runtime_id.as_str()],
    )?;
    Ok(outcome(updated))
}

// Stops prior live runtimes during promotion; no matching runtime is a normal outcome.
pub(crate) fn stop_other_running_runtimes(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    candidate_runtime_id: &RuntimeInstanceId,
) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "UPDATE runtime_instances
              SET state = 'stopped'
              WHERE application_id = ?1
               AND state = 'running'
               AND removed_at IS NULL
               AND id != ?2",
        params![application_id.as_str(), candidate_runtime_id.as_str()],
    )?;
    Ok(())
}

// Reads the logical lifecycle state for cleanup and transition decisions.
pub(crate) fn load_runtime_state(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<Option<RuntimeState>, rusqlite::Error> {
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
}

// Finds another live runtime that may need retirement after candidate promotion.
pub(crate) fn load_previous_runtime(
    connection: &Connection,
    application_id: &ApplicationId,
    candidate_runtime_id: &RuntimeInstanceId,
) -> Result<Option<PreviousRuntime>, rusqlite::Error> {
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
                    runtime_id: entity_id(0, &row.get::<_, String>(0)?)?,
                    deployment_id: entity_id(1, &row.get::<_, String>(1)?)?,
                    external_runtime_id: hydrate_container_id(2, &row.get::<_, String>(2)?)?,
                })
            },
        )
        .optional()
}

// Resolves a logical runtime from the container identity observed from Podman.
pub fn load_runtime_by_external_id(
    connection: &Connection,
    external_runtime_id: &ContainerId,
) -> Result<Option<RuntimeInstance>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT id, application_id, deployment_id, external_runtime_id,
                    state, host_port, container_port,
                    last_observed_state, last_observed_at, exit_code,
                    observation_reason, removed_at
             FROM runtime_instances WHERE external_runtime_id = ?1",
            [external_runtime_id.as_str()],
            map_runtime_instance,
        )
        .optional()
}

// Finds the candidate runtime registered by one deployment without confusing it with an active runtime.
pub(crate) fn load_runtime_by_deployment(
    connection: &Connection,
    deployment_id: &DeploymentId,
) -> Result<Option<RuntimeInstance>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT id, application_id, deployment_id, external_runtime_id,
                    state, host_port, container_port,
                    last_observed_state, last_observed_at, exit_code,
                    observation_reason, removed_at
             FROM runtime_instances WHERE deployment_id = ?1 AND removed_at IS NULL",
            [deployment_id.as_str()],
            map_runtime_instance,
        )
        .optional()
}

// Tombstones only a stopped runtime, preserving lifecycle transition ordering.
// Retirement is the stopped lifecycle state plus explicit `removed_at` evidence.
pub(crate) fn mark_runtime_removed(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PersistenceOutcome, rusqlite::Error> {
    let updated = connection.execute(
        "UPDATE runtime_instances
              SET removed_at = CURRENT_TIMESTAMP
              WHERE id = ?1 AND state = 'stopped' AND removed_at IS NULL",
        [runtime_id.as_str()],
    )?;
    Ok(outcome(updated))
}

// Records failed candidate creation as missing and explicitly retired while it is still starting.
pub(crate) fn mark_starting_runtime_missing(
    connection: &Connection,
    runtime_id: &RuntimeInstanceId,
) -> Result<PersistenceOutcome, rusqlite::Error> {
    let updated = connection.execute(
        "UPDATE runtime_instances
              SET last_observed_state = 'missing',
                  last_observed_at = CURRENT_TIMESTAMP,
                  removed_at = CURRENT_TIMESTAMP
               WHERE id = ?1 AND state = 'starting' AND removed_at IS NULL",
        [runtime_id.as_str()],
    )?;
    Ok(outcome(updated))
}

// Maps persisted runtime identity and enforces the loopback-only endpoint invariant.
fn map_runtime_instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeInstance> {
    let state_text = row.get::<_, String>(4)?;
    let removed_at = row.get::<_, Option<String>>(11)?;
    let state = runtime_state_from_value(&state_text)
        .ok_or_else(|| invalid_text_value(4, "runtime state", &state_text))?;
    // Retirement is explicit removal evidence; it can never coexist with a live
    // running state, matching the schema CHECK.
    let retirement = match (state, removed_at) {
        (RuntimeState::Running, Some(_)) => {
            return Err(invalid_text_value(
                11,
                "running runtime with removed_at",
                &state_text,
            ));
        }
        (_, removed_at) => removed_at.map(|removed_at| RuntimeRetirement { removed_at }),
    };
    let observed_state_text = row.get::<_, String>(7)?;

    Ok(RuntimeInstance {
        id: entity_id(0, &row.get::<_, String>(0)?)?,
        application_id: entity_id(1, &row.get::<_, String>(1)?)?,
        deployment_id: entity_id(2, &row.get::<_, String>(2)?)?,
        external_runtime_id: hydrate_container_id(3, &row.get::<_, String>(3)?)?,
        state,
        expected_endpoint: ExpectedRuntimeEndpoint::new(SocketAddr::from((
            Ipv4Addr::LOCALHOST,
            row.get::<_, u16>(5)?,
        )))
        .map_err(|error| invalid_text_value(5, "runtime endpoint", &error.to_string()))?,
        container_port: ContainerPort::new(row.get::<_, u16>(6)?)
            .map_err(|error| invalid_text_value(6, "runtime container port", &error.to_string()))?,
        observed_state: observed_runtime_state_from_value(&observed_state_text),
        observed_at: row.get(8)?,
        exit_code: row.get(9)?,
        observation_reason: row.get(10)?,
        retirement,
    })
}

// Hydrates a persisted container identity only when it satisfies the domain invariant.
fn hydrate_container_id(column: usize, value: &str) -> rusqlite::Result<ContainerId> {
    if !ContainerId::is_valid(value) {
        return Err(invalid_text_value(column, "external runtime id", value));
    }
    Ok(ContainerId::from(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::params;

    use crate::adapters::database;
    use crate::domain::identity::RuntimeInstanceId;

    use super::*;

    const APP_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const RELEASE_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DEPLOYMENT_ID: &str = "cccccccccccccccccccccccccccccccc";
    const RUNTIME_ID: &str = "dddddddddddddddddddddddddddddddd";
    const OTHER_RUNTIME_ID: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    const SYSTEM_ID: &str = "ffffffffffffffffffffffffffffffff";

    fn runtime_id() -> RuntimeInstanceId {
        RuntimeInstanceId::new(RUNTIME_ID).unwrap()
    }

    fn seed_deployment_chain(connection: &rusqlite::Connection) {
        connection
            .execute_batch(&format!(
                "INSERT INTO systems (id, name) VALUES ('{SYSTEM_ID}', 'team');
                 INSERT INTO applications (
                     id, system_id, name, repository_url, manifest_path, image_repository,
                     container_port, health_check_path, health_check_expected_status,
                     desired_runtime_state
                 ) VALUES (
                     '{APP_ID}', '{SYSTEM_ID}', 'app', 'https://example.test/app.git', 'pneuma.toml',
                     'registry.example/app', 8080, '/healthz', 200, 'stopped');
                 INSERT INTO releases (id, application_id, image_reference, created_at)
                 VALUES ('{RELEASE_ID}', '{APP_ID}',
                         'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                         'now');
                 INSERT INTO deployments (
                     id, application_id, release_id, type, status, requested_at
                 ) VALUES ('{DEPLOYMENT_ID}', '{APP_ID}', '{RELEASE_ID}', 'deploy', 'starting', 'now');"
            ))
            .unwrap();
    }

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
                "INSERT INTO runtime_instances (id, state) VALUES (?1, 'starting')",
                params![RUNTIME_ID],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_instances (id, state) VALUES (?1, 'invalid')",
                params![OTHER_RUNTIME_ID],
            )
            .unwrap();

        assert_eq!(
            load_runtime_state(&connection, &runtime_id()).unwrap(),
            Some(RuntimeState::Starting)
        );
        assert!(matches!(
            load_runtime_state(
                &connection,
                &RuntimeInstanceId::new(OTHER_RUNTIME_ID).unwrap()
            ),
            Err(rusqlite::Error::FromSqlConversionFailure(_, _, _))
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
                 (?1, ?2, ?3, 'not a container id',
                  'running', 30000, 8080, 'running', 'now', NULL, NULL, NULL)",
                params![RUNTIME_ID, APP_ID, DEPLOYMENT_ID],
            )
            .unwrap();

        let error =
            load_runtime_by_external_id(&connection, &ContainerId::from("not a container id"))
                .expect_err("corrupt persisted identity must not hydrate");
        assert!(error.to_string().contains("external runtime id"));
    }

    #[test]
    fn identity_cas_is_stale_unless_the_recorded_container_id_matches() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_instances (
                    id TEXT PRIMARY KEY,
                    external_runtime_id TEXT NOT NULL,
                    last_observed_state TEXT NOT NULL DEFAULT 'running',
                    last_observed_at TEXT NOT NULL DEFAULT '',
                    updated_at TEXT NOT NULL DEFAULT '',
                    removed_at TEXT);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_instances (id, external_runtime_id, removed_at)
                 VALUES (?1, 'current', NULL)",
                params![RUNTIME_ID],
            )
            .unwrap();

        let outcome = reconcile_external_runtime_id(
            &connection,
            &runtime_id(),
            &ContainerId::from("current"),
            &ContainerId::from("replacement"),
        )
        .unwrap();
        assert_eq!(outcome, PersistenceOutcome::Updated);

        let outcome = reconcile_external_runtime_id(
            &connection,
            &runtime_id(),
            &ContainerId::from("current"),
            &ContainerId::from("other"),
        )
        .unwrap();
        assert_eq!(outcome, PersistenceOutcome::Stale);
        let recorded: String = connection
            .query_row(
                "SELECT external_runtime_id FROM runtime_instances WHERE id = ?1",
                params![RUNTIME_ID],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, "replacement");

        connection
            .execute(
                "UPDATE runtime_instances SET removed_at = '2026-01-01' WHERE id = ?1",
                params![RUNTIME_ID],
            )
            .unwrap();
        let outcome = reconcile_external_runtime_id(
            &connection,
            &runtime_id(),
            &ContainerId::from("replacement"),
            &ContainerId::from("next"),
        )
        .unwrap();
        assert_eq!(outcome, PersistenceOutcome::Stale);
    }

    #[test]
    fn retirement_hydration_follows_the_lifecycle_state() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed_deployment_chain(&connection);
        connection
            .execute(
                "INSERT INTO runtime_instances (
                     id, application_id, deployment_id, external_runtime_id, state,
                     host_port, container_port, last_observed_state, last_observed_at
                 ) VALUES (?1, ?2, ?3,
                           'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                           'stopped', 30001, 8080, 'stopped', 'now')",
                params![RUNTIME_ID, APP_ID, DEPLOYMENT_ID],
            )
            .unwrap();

        // A stopped runtime without removal evidence is not retired.
        let runtime = load_runtime_by_external_id(
            &connection,
            &ContainerId::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(runtime.state, RuntimeState::Stopped);
        assert_eq!(runtime.retirement, None);

        // Retirement is `removed_at` evidence on the stopped lifecycle state.
        connection
            .execute(
                "UPDATE runtime_instances SET removed_at = '2026-01-01' WHERE id = ?1",
                params![RUNTIME_ID],
            )
            .unwrap();
        let runtime = load_runtime_by_external_id(
            &connection,
            &ContainerId::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(runtime.state, RuntimeState::Stopped);
        assert_eq!(
            runtime.retirement.map(|retirement| retirement.removed_at),
            Some("2026-01-01".to_owned())
        );

        // A running runtime can never carry retirement evidence; the CHECK
        // constraint is bypassed so hydration must refuse the contradiction.
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_instances (
                     id, application_id, deployment_id, external_runtime_id, state,
                     host_port, container_port, last_observed_state, last_observed_at,
                     removed_at
                 ) VALUES (?1, ?2, ?3,
                           'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                           'running', 30002, 8080, 'running', 'now', '2026-01-01')",
                params![OTHER_RUNTIME_ID, APP_ID, DEPLOYMENT_ID],
            )
            .unwrap();
        let error = load_runtime_by_external_id(
            &connection,
            &ContainerId::from("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("removed_at"));
    }
}
