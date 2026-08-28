use std::env;

use rusqlite::{Connection, TransactionBehavior, params};
use thiserror::Error;

use crate::domain::identity::{ApplicationId, DeploymentId};
use crate::domain::runtime::HostPort;

pub(crate) const RUNTIME_PORT_RANGE_ENVIRONMENT_VARIABLE: &str = "PNEUMA_RUNTIME_PORT_RANGE";
const DEFAULT_RUNTIME_PORT_RANGE: &str = "30000-39999";

#[derive(Debug, Error)]
pub enum PortAllocationError {
    #[error(
        "{RUNTIME_PORT_RANGE_ENVIRONMENT_VARIABLE} must be <start>-<end> within 1-65535, got `{value}`"
    )]
    InvalidRange { value: String },
    #[error("no free runtime port is available in {start}-{end}")]
    Exhausted { start: u16, end: u16 },
    #[error("failed to allocate runtime port: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

// Atomically reserves the first free configured loopback port across live runtimes and candidates.
pub(crate) fn reserve_port(
    connection: &mut Connection,
    application_id: &ApplicationId,
    deployment_id: &DeploymentId,
) -> Result<HostPort, PortAllocationError> {
    let (start, end) = configured_range()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| PortAllocationError::Persistence { source })?;
    for raw in start..=end {
        // The configured range already excludes zero; skipping keeps the invariant local.
        let Ok(port) = HostPort::new(raw) else {
            continue;
        };
        let in_use: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM runtime_instances
                    WHERE host_address = '127.0.0.1' AND host_port = ?1 AND removed_at IS NULL
                    UNION ALL
                    SELECT 1 FROM runtime_port_reservations WHERE port = ?1
                )",
                [port.get()],
                |row| row.get(0),
            )
            .map_err(|source| PortAllocationError::Persistence { source })?;
        if in_use {
            continue;
        }
        transaction
            .execute(
                "INSERT INTO runtime_port_reservations (port, application_id, deployment_id)
                 VALUES (?1, ?2, ?3)",
                params![port.get(), application_id.as_str(), deployment_id.as_str()],
            )
            .map_err(|source| PortAllocationError::Persistence { source })?;
        transaction
            .commit()
            .map_err(|source| PortAllocationError::Persistence { source })?;
        return Ok(port);
    }
    Err(PortAllocationError::Exhausted { start, end })
}

// Releases all reservations owned by a deployment after cleanup or runtime registration.
pub(crate) fn release_port(
    connection: &Connection,
    deployment_id: &DeploymentId,
) -> Result<(), PortAllocationError> {
    connection
        .execute(
            "DELETE FROM runtime_port_reservations WHERE deployment_id = ?1",
            [deployment_id.as_str()],
        )
        .map_err(|source| PortAllocationError::Persistence { source })?;
    Ok(())
}

// Consumes a reservation once its port is recorded on a RuntimeInstance.
pub(crate) fn consume_port_reservation(
    connection: &Connection,
    deployment_id: &DeploymentId,
) -> Result<(), PortAllocationError> {
    release_port(connection, deployment_id)
}

// Parses the host-configured allocation range.
fn configured_range() -> Result<(u16, u16), PortAllocationError> {
    let value = env::var(RUNTIME_PORT_RANGE_ENVIRONMENT_VARIABLE)
        .unwrap_or_else(|_| DEFAULT_RUNTIME_PORT_RANGE.to_owned());
    parse_range(&value)
}

// Parses an allocation range and rejects zero, inverted, or malformed bounds.
fn parse_range(value: &str) -> Result<(u16, u16), PortAllocationError> {
    let invalid = || PortAllocationError::InvalidRange {
        value: value.to_owned(),
    };
    let Some((start, end)) = value.split_once('-') else {
        return Err(invalid());
    };
    let (Ok(start), Ok(end)) = (start.parse::<u16>(), end.parse::<u16>()) else {
        return Err(invalid());
    };
    if start == 0 || end == 0 || start > end {
        return Err(invalid());
    }
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::ErrorCode;

    use crate::adapters::database;
    use crate::domain::identity::{ApplicationId, DeploymentId};

    use super::{PortAllocationError, parse_range, release_port, reserve_port};

    #[test]
    fn rejects_malformed_zero_and_inverted_ranges() {
        for value in ["", "30000", "abc-def", "0-100", "100-0", "30000-30000-1"] {
            assert!(
                matches!(
                    parse_range(value),
                    Err(PortAllocationError::InvalidRange { .. })
                ),
                "expected `{value}` to be rejected"
            );
        }
        assert_eq!(parse_range("30000-39999").unwrap(), (30000, 39999));
        assert_eq!(parse_range("1-65535").unwrap(), (1, 65535));
    }

    #[test]
    fn reserves_distinct_ports_and_reuses_a_released_port() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        seed_runtime_inputs(&connection);

        let first = reserve_port(&mut connection, &application(), &deployment()).unwrap();
        let second = reserve_port(&mut connection, &application(), &deployment()).unwrap();
        assert_eq!(first.get(), 30000);
        assert_eq!(second.get(), 30001);

        release_port(&connection, &deployment()).unwrap();
        let reused = reserve_port(&mut connection, &application(), &deployment()).unwrap();
        assert_eq!(reused.get(), 30000);
    }

    #[test]
    fn skips_live_runtime_endpoints_and_reuses_removed_ones() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        seed_runtime_inputs(&connection);
        connection
            .execute(
                "INSERT INTO runtime_instances (
                     id, application_id, deployment_id, external_runtime_id, state,
                     host_address, host_port, container_port, last_observed_state,
                     last_observed_at, created_at, updated_at, removed_at
                 ) VALUES ('55555555555555555555555555555555', '11111111111111111111111111111111', '22222222222222222222222222222222', 'aabbccdd', 'running',
                           '127.0.0.1', 30000, 8080, 'running',
                           'now', 'now', 'now', NULL)",
                [],
            )
            .unwrap();

        let reserved = reserve_port(&mut connection, &application(), &deployment()).unwrap();
        assert_eq!(reserved.get(), 30001);

        connection
            .execute(
                "UPDATE runtime_instances SET removed_at = 'later' WHERE id = '55555555555555555555555555555555'",
                [],
            )
            .unwrap();
        release_port(&connection, &deployment()).unwrap();
        let freed = reserve_port(&mut connection, &application(), &deployment()).unwrap();
        assert_eq!(freed.get(), 30000);
    }

    #[test]
    fn reports_exhaustion_when_every_configured_port_is_reserved() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        seed_runtime_inputs(&connection);
        let mut insert = connection
            .prepare(
                "INSERT INTO runtime_port_reservations (port, application_id, deployment_id)
                 VALUES (?1, '11111111111111111111111111111111', '22222222222222222222222222222222')",
            )
            .unwrap();
        for port in 30000..=39999u16 {
            insert.execute([port]).unwrap();
        }
        drop(insert);

        let error = reserve_port(&mut connection, &application(), &deployment()).unwrap_err();

        assert!(matches!(
            error,
            PortAllocationError::Exhausted {
                start: 30000,
                end: 39999
            }
        ));
    }

    #[test]
    fn duplicate_reservations_are_rejected_by_the_primary_key() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        seed_runtime_inputs(&connection);
        reserve_port(&mut connection, &application(), &deployment()).unwrap();

        let error = connection
            .execute(
                "INSERT INTO runtime_port_reservations (port, application_id, deployment_id)
                 VALUES (30000, '11111111111111111111111111111111', '22222222222222222222222222222222')",
                [],
            )
            .unwrap_err();

        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == ErrorCode::ConstraintViolation
        ));
    }

    fn application() -> ApplicationId {
        ApplicationId::new("11111111111111111111111111111111").unwrap()
    }

    fn deployment() -> DeploymentId {
        DeploymentId::new("22222222222222222222222222222222").unwrap()
    }

    // Seeds the application, release, and deployment rows required by reservation foreign keys.
    fn seed_runtime_inputs(connection: &rusqlite::Connection) {
        connection
            .execute_batch(
                "INSERT INTO systems (id, name, created_at) VALUES ('33333333333333333333333333333333', 'team', 'now');
                 INSERT INTO applications (id, system_id, name, desired_runtime_state, created_at, updated_at)
                 VALUES ('11111111111111111111111111111111', '33333333333333333333333333333333', 'app', 'stopped', 'now', 'now');
                 INSERT INTO releases (id, application_id, image_repository, image_digest, image_reference, created_at)
                 VALUES ('44444444444444444444444444444444', '11111111111111111111111111111111', 'registry.example/app',
                         'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                         'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                         'now');
                 INSERT INTO deployments (id, application_id, release_id, type, status, requested_at)
                 VALUES ('22222222222222222222222222222222', '11111111111111111111111111111111', '44444444444444444444444444444444', 'deploy', 'pending', 'now');",
            )
            .unwrap();
    }
}
