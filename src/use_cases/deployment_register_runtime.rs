use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::domain::deployment::DeploymentStatus;
use crate::domain::identity::RuntimeInstanceId;
use crate::domain::runtime::{
    ContainerId, ContainerPort, ExpectedRuntimeEndpoint, RuntimeInstance, RuntimeRegistration,
};

#[derive(Debug)]
pub enum RegisterCandidateRuntimeError {
    InvalidExternalRuntimeId,
    InvalidEndpoint {
        endpoint: SocketAddr,
    },
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
    RegistrationNotFound {
        runtime_id: RuntimeInstanceId,
    },
    Store {
        source: RuntimeStoreError,
    },
    DeploymentStore {
        source: DeploymentStoreError,
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
            Self::RegistrationNotFound { runtime_id } => {
                write!(
                    formatter,
                    "registered runtime `{runtime_id}` could not be reloaded"
                )
            }
            Self::Store { source } => {
                write!(formatter, "failed to register candidate runtime: {source}")
            }
            Self::DeploymentStore { source } => {
                write!(formatter, "failed to register candidate runtime: {source}")
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
            Self::Store { source } => Some(source),
            Self::DeploymentStore { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::InvalidExternalRuntimeId
            | Self::InvalidEndpoint { .. }
            | Self::DeploymentNotFound { .. }
            | Self::InvalidDeploymentState { .. }
            | Self::ExternalRuntimeConflict { .. }
            | Self::EndpointConflict { .. } => None,
            Self::RegistrationNotFound { .. } => None,
        }
    }
}

impl From<DeploymentStoreError> for RegisterCandidateRuntimeError {
    fn from(error: DeploymentStoreError) -> Self {
        match error {
            DeploymentStoreError::NotFound { deployment_id } => {
                Self::DeploymentNotFound { deployment_id }
            }
            DeploymentStoreError::Stale { deployment_id } => Self::InvalidDeploymentState {
                deployment_id,
                actual: "changed before persistence".to_owned(),
            },
            DeploymentStoreError::InvalidStatus {
                deployment_id,
                status,
            } => Self::InvalidDeploymentState {
                deployment_id,
                actual: status,
            },
            DeploymentStoreError::InvalidType { .. } => Self::DeploymentStore { source: error },
            DeploymentStoreError::InvalidEvidence { .. } => Self::DeploymentStore { source: error },
            DeploymentStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

impl From<RuntimeStoreError> for RegisterCandidateRuntimeError {
    fn from(error: RuntimeStoreError) -> Self {
        match error {
            RuntimeStoreError::Persistence { source } => Self::Persistence { source },
        }
    }
}

// Registers an observed candidate in one transaction after validating loopback identity.
pub fn register_candidate_runtime(
    connection: &mut Connection,
    deployment_id: &str,
    external_runtime_id: &ContainerId,
    endpoint: SocketAddr,
    container_port: ContainerPort,
) -> Result<RuntimeInstance, RegisterCandidateRuntimeError> {
    validate_runtime(external_runtime_id.as_str(), endpoint)?;

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;
    if let Some(existing) =
        runtime_store::load_runtime_by_external_id(&transaction, external_runtime_id.as_str())?
    {
        if existing.deployment_id.as_str() == deployment_id
            && existing.expected_endpoint.socket_addr() == endpoint
            && existing.container_port == container_port.get()
        {
            transaction
                .commit()
                .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;
            return Ok(existing);
        }
        return Err(RegisterCandidateRuntimeError::ExternalRuntimeConflict {
            external_runtime_id: external_runtime_id.to_string(),
        });
    }

    let deployment = deployment_store::load_deployment(&transaction, deployment_id)?;
    if deployment.status() != DeploymentStatus::Starting {
        return Err(RegisterCandidateRuntimeError::InvalidDeploymentState {
            deployment_id: deployment_id.to_owned(),
            actual: deployment.status().to_string(),
        });
    }

    let port_reserved =
        runtime_store::port_is_reserved(&transaction, "127.0.0.1", endpoint.port())?;
    if port_reserved {
        return Err(RegisterCandidateRuntimeError::EndpointConflict { endpoint });
    }

    let runtime_id = runtime_store::generate_id(&transaction)?;
    let registration = RuntimeRegistration {
        id: RuntimeInstanceId::from(runtime_id),
        application_id: deployment.application_id,
        deployment_id: deployment.id,
        external_runtime_id: external_runtime_id.clone(),
        expected_endpoint: ExpectedRuntimeEndpoint::new(endpoint)
            .map_err(|_| RegisterCandidateRuntimeError::InvalidEndpoint { endpoint })?,
        container_port: container_port.get(),
    };
    runtime_store::insert_runtime(&transaction, &registration)?;

    let runtime =
        runtime_store::load_runtime_by_external_id(&transaction, external_runtime_id.as_str())?
            .ok_or_else(|| RegisterCandidateRuntimeError::RegistrationNotFound {
                runtime_id: registration.id.clone(),
            })?;
    transaction
        .commit()
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;

    Ok(runtime)
}

// Enforces the external-ID and loopback endpoint invariants before persistence.
fn validate_runtime(
    external_runtime_id: &str,
    endpoint: SocketAddr,
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
    Ok(())
}
