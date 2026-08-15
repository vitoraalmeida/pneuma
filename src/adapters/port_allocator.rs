use std::env;
use std::error::Error;
use std::fmt;

use rusqlite::{Connection, TransactionBehavior, params};

pub const RUNTIME_PORT_RANGE_ENVIRONMENT_VARIABLE: &str = "PNEUMA_RUNTIME_PORT_RANGE";
const DEFAULT_RUNTIME_PORT_RANGE: &str = "30000-39999";

#[derive(Debug)]
pub enum PortAllocationError {
    InvalidRange { value: String },
    Exhausted { start: u16, end: u16 },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for PortAllocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRange { value } => write!(
                formatter,
                "{RUNTIME_PORT_RANGE_ENVIRONMENT_VARIABLE} must be <start>-<end> within 1-65535, got `{value}`"
            ),
            Self::Exhausted { start, end } => {
                write!(
                    formatter,
                    "no free runtime port is available in {start}-{end}"
                )
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to allocate runtime port: {source}")
            }
        }
    }
}

impl Error for PortAllocationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::InvalidRange { .. } | Self::Exhausted { .. } => None,
        }
    }
}

// Atomically reserves the first free configured loopback port across live runtimes and candidates.
pub fn reserve_port(
    connection: &mut Connection,
    application_id: &str,
    deployment_id: &str,
) -> Result<u16, PortAllocationError> {
    let (start, end) = configured_range()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| PortAllocationError::Persistence { source })?;
    for port in start..=end {
        let in_use: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM runtime_instances
                    WHERE host_address = '127.0.0.1' AND host_port = ?1 AND removed_at IS NULL
                    UNION ALL
                    SELECT 1 FROM runtime_port_reservations WHERE port = ?1
                )",
                [port],
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
                params![port, application_id, deployment_id],
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
pub fn release_port(
    connection: &Connection,
    deployment_id: &str,
) -> Result<(), PortAllocationError> {
    connection
        .execute(
            "DELETE FROM runtime_port_reservations WHERE deployment_id = ?1",
            [deployment_id],
        )
        .map_err(|source| PortAllocationError::Persistence { source })?;
    Ok(())
}

// Consumes a reservation once its port is recorded on a RuntimeInstance.
pub fn consume_port_reservation(
    connection: &Connection,
    deployment_id: &str,
) -> Result<(), PortAllocationError> {
    release_port(connection, deployment_id)
}

// Parses the host-configured allocation range and rejects zero, inverted, or malformed bounds.
fn configured_range() -> Result<(u16, u16), PortAllocationError> {
    let value = env::var(RUNTIME_PORT_RANGE_ENVIRONMENT_VARIABLE)
        .unwrap_or_else(|_| DEFAULT_RUNTIME_PORT_RANGE.to_owned());
    let Some((start, end)) = value.split_once('-') else {
        return Err(PortAllocationError::InvalidRange { value });
    };
    let (Ok(start), Ok(end)) = (start.parse::<u16>(), end.parse::<u16>()) else {
        return Err(PortAllocationError::InvalidRange { value });
    };
    if start == 0 || end == 0 || start > end {
        return Err(PortAllocationError::InvalidRange { value });
    }
    Ok((start, end))
}
