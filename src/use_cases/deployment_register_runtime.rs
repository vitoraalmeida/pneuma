use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::adapters::local_runtime::ObservedRuntimeState;

#[derive(Debug, PartialEq, Eq)]
pub struct CandidateRuntime {
    pub id: String,
    pub application_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
    pub endpoint: SocketAddr,
    pub container_port: u16,
    pub observed_state: ObservedRuntimeState,
    pub observed_at: String,
}

#[derive(Debug)]
pub enum RegisterCandidateRuntimeError {
    InvalidExternalRuntimeId,
    InvalidEndpoint {
        endpoint: SocketAddr,
    },
    InvalidContainerPort,
    DeploymentNotFound {
        deployment_id: String,
    },
    InvalidDeploymentState {
        deployment_id: String,
        actual: String,
    },
    ExternalRuntimeConflict {
        external_runtime_id: String,
    },
    EndpointConflict {
        endpoint: SocketAddr,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for RegisterCandidateRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExternalRuntimeId => {
                formatter.write_str("external runtime ID must be a non-empty hexadecimal value")
            }
            Self::InvalidEndpoint { endpoint } => write!(
                formatter,
                "candidate runtime endpoint must be IPv4 loopback with a nonzero port: {endpoint}"
            ),
            Self::InvalidContainerPort => {
                formatter.write_str("container port must be between 1 and 65535")
            }
            Self::DeploymentNotFound { deployment_id } => {
                write!(formatter, "deployment `{deployment_id}` was not found")
            }
            Self::InvalidDeploymentState {
                deployment_id,
                actual,
            } => write!(
                formatter,
                "deployment `{deployment_id}` must be Starting to register a candidate, but is `{actual}`"
            ),
            Self::ExternalRuntimeConflict {
                external_runtime_id,
            } => write!(
                formatter,
                "external runtime `{external_runtime_id}` is already registered with different data"
            ),
            Self::EndpointConflict { endpoint } => {
                write!(formatter, "runtime endpoint `{endpoint}` is already active")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to register candidate runtime: {source}")
            }
        }
    }
}

impl Error for RegisterCandidateRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::InvalidExternalRuntimeId
            | Self::InvalidEndpoint { .. }
            | Self::InvalidContainerPort
            | Self::DeploymentNotFound { .. }
            | Self::InvalidDeploymentState { .. }
            | Self::ExternalRuntimeConflict { .. }
            | Self::EndpointConflict { .. } => None,
        }
    }
}

pub fn register_candidate_runtime(
    connection: &mut Connection,
    deployment_id: &str,
    external_runtime_id: &str,
    endpoint: SocketAddr,
    container_port: u16,
) -> Result<CandidateRuntime, RegisterCandidateRuntimeError> {
    validate_runtime(external_runtime_id, endpoint, container_port)?;

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;
    if let Some(existing) = load_by_external_id(&transaction, external_runtime_id)? {
        if existing.deployment_id == deployment_id
            && existing.endpoint == endpoint
            && existing.container_port == container_port
        {
            transaction
                .commit()
                .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;
            return Ok(existing);
        }
        return Err(RegisterCandidateRuntimeError::ExternalRuntimeConflict {
            external_runtime_id: external_runtime_id.to_owned(),
        });
    }

    let deployment = transaction
        .query_row(
            "SELECT application_id, status FROM deployments WHERE id = ?1",
            [deployment_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?
        .ok_or_else(|| RegisterCandidateRuntimeError::DeploymentNotFound {
            deployment_id: deployment_id.to_owned(),
        })?;
    if deployment.1 != "starting" {
        return Err(RegisterCandidateRuntimeError::InvalidDeploymentState {
            deployment_id: deployment_id.to_owned(),
            actual: deployment.1,
        });
    }

    let endpoint_registered = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM runtime_instances
                WHERE host_address = '127.0.0.1'
                  AND host_port = ?1
                  AND removed_at IS NULL
             )",
            [endpoint.port()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;
    if endpoint_registered {
        return Err(RegisterCandidateRuntimeError::EndpointConflict { endpoint });
    }

    transaction
        .execute(
            "INSERT INTO runtime_instances (
                id,
                application_id,
                deployment_id,
                external_runtime_id,
                state,
                host_address,
                host_port,
                container_port,
                last_observed_state,
                last_observed_at
             ) VALUES (
                lower(hex(randomblob(16))),
                ?1, ?2, ?3, 'starting', '127.0.0.1', ?4, ?5,
                'running', CURRENT_TIMESTAMP
             )",
            params![
                deployment.0,
                deployment_id,
                external_runtime_id,
                endpoint.port(),
                container_port
            ],
        )
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;
    let runtime = load_by_external_id(&transaction, external_runtime_id)?.ok_or_else(|| {
        RegisterCandidateRuntimeError::Persistence {
            source: rusqlite::Error::QueryReturnedNoRows,
        }
    })?;
    transaction
        .commit()
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;

    Ok(runtime)
}

fn validate_runtime(
    external_runtime_id: &str,
    endpoint: SocketAddr,
    container_port: u16,
) -> Result<(), RegisterCandidateRuntimeError> {
    if external_runtime_id.is_empty()
        || !external_runtime_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RegisterCandidateRuntimeError::InvalidExternalRuntimeId);
    }
    if endpoint.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || endpoint.port() == 0 {
        return Err(RegisterCandidateRuntimeError::InvalidEndpoint { endpoint });
    }
    if container_port == 0 {
        return Err(RegisterCandidateRuntimeError::InvalidContainerPort);
    }

    Ok(())
}

fn load_by_external_id(
    connection: &Connection,
    external_runtime_id: &str,
) -> Result<Option<CandidateRuntime>, RegisterCandidateRuntimeError> {
    connection
        .query_row(
            "SELECT
                id,
                application_id,
                deployment_id,
                external_runtime_id,
                host_port,
                container_port,
                last_observed_at
             FROM runtime_instances
             WHERE external_runtime_id = ?1",
            [external_runtime_id],
            |row| {
                let host_port = row.get::<_, u16>(4)?;
                Ok(CandidateRuntime {
                    id: row.get(0)?,
                    application_id: row.get(1)?,
                    deployment_id: row.get(2)?,
                    external_runtime_id: row.get(3)?,
                    endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, host_port)),
                    container_port: row.get(5)?,
                    observed_state: ObservedRuntimeState::Running,
                    observed_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })
}
