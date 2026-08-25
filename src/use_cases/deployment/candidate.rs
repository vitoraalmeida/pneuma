use std::error::Error;
use std::net::SocketAddr;

use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

use super::cleanup::CandidateResources;
use super::transition::{TransitionDeploymentError, advance_deployment};
use crate::adapters::local_runtime::{observe_container, resolve_container_id};
use crate::adapters::port_allocator::{consume_port_reservation, reserve_port};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::adapters::systemd_quadlet::{
    QuadletError, container_name, daemon_reload, start, write_unit,
};
use crate::domain::application::ApplicationName;
use crate::domain::deployment::{DeploymentEvent, DeploymentStatus};
use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId};
use crate::domain::release::OciArtifact;
use crate::domain::runtime::{
    ContainerId, ContainerPort, ExpectedRuntimeEndpoint, HostPort, ObservedRuntimeState,
    RuntimeInstance, RuntimeRegistration, RuntimeSpecification,
};

// Returns the observed candidate identity needed by verification and cleanup orchestration.
pub(crate) struct StartedCandidate {
    pub(crate) runtime: RuntimeInstance,
    pub(crate) container_name: String,
    pub(crate) unit_name: String,
    pub(crate) port: HostPort,
}

// Groups the persisted deployment context and immutable artifact inputs for candidate startup.
pub(crate) struct CandidateStartInput<'a> {
    pub(crate) connection: &'a mut Connection,
    pub(crate) deployment_id: &'a DeploymentId,
    pub(crate) application_id: &'a ApplicationId,
    pub(crate) application_name: &'a ApplicationName,
    pub(crate) artifact: &'a OciArtifact,
    pub(crate) runtime: &'a RuntimeSpecification,
}

pub(crate) enum CandidateStartError {
    PortAllocation {
        source: crate::adapters::port_allocator::PortAllocationError,
    },
    UnitCreation {
        source: QuadletError,
        resources: Box<CandidateResources>,
    },
    UnitReload {
        source: QuadletError,
        resources: Box<CandidateResources>,
    },
    UnitStart {
        source: QuadletError,
        resources: Box<CandidateResources>,
    },
    ContainerResolution {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    ContainerObservation {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    RuntimeRegistration {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    PortPersistence {
        source: crate::adapters::port_allocator::PortAllocationError,
        resources: Box<CandidateResources>,
    },
    DeploymentTransition {
        source: TransitionDeploymentError,
        resources: Box<CandidateResources>,
    },
}

#[derive(Debug, Error)]
enum RuntimeObservationFailure {
    #[error("expected runtime to be Running, got {actual:?}")]
    NotRunning { actual: ObservedRuntimeState },
    #[error("running runtime has no loopback endpoint")]
    MissingEndpoint,
    #[error("running runtime has an invalid loopback endpoint")]
    InvalidEndpoint,
}

// Materializes a candidate in ordered external steps, retaining resources for compensation on failure.
pub(crate) fn start_candidate(
    input: CandidateStartInput<'_>,
) -> Result<StartedCandidate, CandidateStartError> {
    let CandidateStartInput {
        connection,
        deployment_id,
        application_id,
        application_name,
        artifact,
        runtime,
    } = input;

    advance_deployment(connection, deployment_id, DeploymentEvent::Start).map_err(|source| {
        CandidateStartError::DeploymentTransition {
            source,
            resources: Box::new(CandidateResources::empty()),
        }
    })?;

    let host_port = reserve_port(connection, application_id, deployment_id)
        .map_err(|source| CandidateStartError::PortAllocation { source })?;
    let mut resources = CandidateResources::empty().with_port();

    let unit = write_unit(
        application_name,
        deployment_id,
        artifact,
        runtime.container_port(),
        host_port,
    )
    .map_err(|source| CandidateStartError::UnitCreation {
        source,
        resources: Box::new(resources.clone()),
    })?;
    resources = resources.with_unit(&unit);

    daemon_reload().map_err(|source| CandidateStartError::UnitReload {
        source,
        resources: Box::new(resources.clone()),
    })?;

    start(&unit).map_err(|source| CandidateStartError::UnitStart {
        source,
        resources: Box::new(resources.clone()),
    })?;

    let name = container_name(application_name, deployment_id);
    let container_id =
        resolve_container_id(&name).map_err(|source| CandidateStartError::ContainerResolution {
            source: Box::new(source),
            resources: Box::new(resources.clone()),
        })?;
    resources = resources.with_container_mut(&container_id);

    let observation =
        observe_container(&container_id, runtime.container_port()).map_err(|source| {
            CandidateStartError::ContainerObservation {
                source: Box::new(source),
                resources: Box::new(resources.clone()),
            }
        })?;

    if *observation.state() != ObservedRuntimeState::Running {
        return Err(CandidateStartError::ContainerObservation {
            source: Box::new(RuntimeObservationFailure::NotRunning {
                actual: observation.state().clone(),
            }),
            resources: Box::new(resources.clone()),
        });
    }

    let endpoint = observation.observed_endpoint().ok_or_else(|| {
        CandidateStartError::ContainerObservation {
            source: Box::new(RuntimeObservationFailure::MissingEndpoint),
            resources: Box::new(resources.clone()),
        }
    })?;
    let endpoint = ExpectedRuntimeEndpoint::new(endpoint).map_err(|_| {
        CandidateStartError::ContainerObservation {
            source: Box::new(RuntimeObservationFailure::InvalidEndpoint),
            resources: Box::new(resources.clone()),
        }
    })?;

    let runtime = register_candidate_runtime(
        connection,
        deployment_id,
        &container_id,
        endpoint,
        runtime.container_port(),
    )
    .map_err(|source| CandidateStartError::RuntimeRegistration {
        source: Box::new(source),
        resources: Box::new(resources.clone()),
    })?;
    resources = resources.with_runtime_mut(&runtime.id);

    consume_port_reservation(connection, deployment_id).map_err(|source| {
        CandidateStartError::PortPersistence {
            source,
            resources: Box::new(resources.clone()),
        }
    })?;

    advance_deployment(connection, deployment_id, DeploymentEvent::RuntimeRunning).map_err(
        |source| CandidateStartError::DeploymentTransition {
            source,
            resources: Box::new(resources.clone()),
        },
    )?;

    Ok(StartedCandidate {
        runtime,
        container_name: name,
        unit_name: unit,
        port: host_port,
    })
}

#[derive(Debug, Error)]
pub enum RegisterCandidateRuntimeError {
    #[error("external runtime ID must be a non-empty hexadecimal value")]
    InvalidExternalRuntimeId,
    #[error("deployment `{deployment_id}` was not found")]
    DeploymentNotFound { deployment_id: String },
    #[error(
        "deployment `{deployment_id}` must be Starting to register a candidate, but is `{actual}`"
    )]
    InvalidDeploymentState {
        deployment_id: String,
        actual: String,
    },
    #[error("external runtime `{external_runtime_id}` is already registered with different data")]
    ExternalRuntimeConflict { external_runtime_id: String },
    #[error("runtime endpoint `{endpoint}` is already active")]
    EndpointConflict { endpoint: SocketAddr },
    #[error("registered runtime `{runtime_id}` could not be reloaded")]
    RegistrationNotFound { runtime_id: RuntimeInstanceId },
    #[error("failed to register candidate runtime: {source}")]
    Store {
        #[source]
        source: RuntimeStoreError,
    },
    #[error("failed to register candidate runtime: {source}")]
    DeploymentStore {
        #[source]
        source: DeploymentStoreError,
    },
    #[error("failed to register candidate runtime: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
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
    deployment_id: &DeploymentId,
    external_runtime_id: &ContainerId,
    endpoint: ExpectedRuntimeEndpoint,
    container_port: ContainerPort,
) -> Result<RuntimeInstance, RegisterCandidateRuntimeError> {
    validate_external_runtime_id(external_runtime_id.as_str())?;

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;
    if let Some(existing) =
        runtime_store::load_runtime_by_external_id(&transaction, external_runtime_id)?
    {
        if matches_existing_registration(&existing, deployment_id, endpoint, container_port) {
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
            deployment_id: deployment_id.to_string(),
            actual: deployment.status().to_string(),
        });
    }

    let port_reserved = runtime_store::port_is_reserved(&transaction, &endpoint)?;
    if port_reserved {
        return Err(RegisterCandidateRuntimeError::EndpointConflict {
            endpoint: endpoint.socket_addr(),
        });
    }

    let runtime_id = runtime_store::generate_id(&transaction)?;
    let registration = RuntimeRegistration {
        id: runtime_id,
        application_id: deployment.application_id,
        deployment_id: deployment.id,
        external_runtime_id: external_runtime_id.clone(),
        expected_endpoint: endpoint,
        container_port,
    };
    runtime_store::insert_runtime(&transaction, &registration)?;

    let runtime = runtime_store::load_runtime_by_external_id(&transaction, external_runtime_id)?
        .ok_or_else(|| RegisterCandidateRuntimeError::RegistrationNotFound {
            runtime_id: registration.id.clone(),
        })?;
    transaction
        .commit()
        .map_err(|source| RegisterCandidateRuntimeError::Persistence { source })?;

    Ok(runtime)
}

// A runtime already registered with the identical deployment, endpoint, and port
// makes re-registering the same external container idempotent instead of conflicting.
fn matches_existing_registration(
    existing: &RuntimeInstance,
    deployment_id: &DeploymentId,
    endpoint: ExpectedRuntimeEndpoint,
    container_port: ContainerPort,
) -> bool {
    existing.deployment_id == *deployment_id
        && existing.expected_endpoint == endpoint
        && existing.container_port == container_port
}

// Enforces the external container-ID invariant before persistence.
fn validate_external_runtime_id(
    external_runtime_id: &str,
) -> Result<(), RegisterCandidateRuntimeError> {
    if !ContainerId::is_valid(external_runtime_id) {
        return Err(RegisterCandidateRuntimeError::InvalidExternalRuntimeId);
    }
    Ok(())
}
