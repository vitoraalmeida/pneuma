use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::local_runtime::{observe_container, resolve_container_id};
use crate::adapters::port_allocator::{consume_port_reservation, reserve_port};
use crate::adapters::systemd_quadlet::{
    QuadletError, container_name, daemon_reload, start, write_unit,
};
use crate::domain::runtime::{ObservedRuntimeState, RuntimeInstance};
use crate::use_cases::deployment_register_runtime::register_candidate_runtime;
use crate::use_cases::deployment_runtime_cleanup::CandidateResources;
use crate::use_cases::deployment_transition::{
    DeploymentTransition, TransitionDeploymentError, advance_deployment,
};

// Returns the observed candidate identity needed by verification and cleanup orchestration.
pub(crate) struct StartedCandidate {
    pub runtime: RuntimeInstance,
    pub container_name: String,
    pub unit_name: String,
    pub port: u16,
}

// Groups the persisted deployment context and immutable artifact inputs for candidate startup.
pub(crate) struct CandidateStartInput<'a> {
    pub connection: &'a mut Connection,
    pub deployment_id: &'a str,
    pub application_id: &'a str,
    pub application_name: &'a str,
    pub image_reference: &'a str,
    pub container_port: u16,
    pub artifact_identity: &'a str,
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

#[derive(Debug)]
enum RuntimeObservationFailure {
    NotRunning { actual: ObservedRuntimeState },
    MissingEndpoint,
}

impl fmt::Display for RuntimeObservationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning { actual } => {
                write!(formatter, "expected runtime to be Running, got {actual:?}")
            }
            Self::MissingEndpoint => {
                formatter.write_str("running runtime has no loopback endpoint")
            }
        }
    }
}

impl Error for RuntimeObservationFailure {}

// Materializes a candidate in ordered external steps, retaining resources for compensation on failure.
pub(crate) fn start_candidate(
    input: CandidateStartInput<'_>,
) -> Result<StartedCandidate, CandidateStartError> {
    let CandidateStartInput {
        connection,
        deployment_id,
        application_id,
        application_name,
        image_reference,
        container_port,
        artifact_identity,
    } = input;

    advance_deployment(connection, deployment_id, DeploymentTransition::Start).map_err(
        |source| CandidateStartError::DeploymentTransition {
            source,
            resources: Box::new(CandidateResources::empty()),
        },
    )?;

    let host_port = reserve_port(connection, application_id, deployment_id)
        .map_err(|source| CandidateStartError::PortAllocation { source })?;
    let mut resources = CandidateResources::empty().with_port();

    let unit = write_unit(
        application_name,
        deployment_id,
        image_reference,
        container_port,
        host_port,
        artifact_identity,
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

    let observation = observe_container(&container_id, container_port).map_err(|source| {
        CandidateStartError::ContainerObservation {
            source: Box::new(source),
            resources: Box::new(resources.clone()),
        }
    })?;

    if observation.state != ObservedRuntimeState::Running {
        return Err(CandidateStartError::ContainerObservation {
            source: Box::new(RuntimeObservationFailure::NotRunning {
                actual: observation.state,
            }),
            resources: Box::new(resources.clone()),
        });
    }

    let endpoint =
        observation
            .endpoint
            .ok_or_else(|| CandidateStartError::ContainerObservation {
                source: Box::new(RuntimeObservationFailure::MissingEndpoint),
                resources: Box::new(resources.clone()),
            })?;

    let runtime = register_candidate_runtime(
        connection,
        deployment_id,
        &container_id,
        endpoint,
        container_port,
    )
    .map_err(|source| CandidateStartError::RuntimeRegistration {
        source: Box::new(source),
        resources: Box::new(resources.clone()),
    })?;
    resources = resources.with_runtime_mut(runtime.id.as_str());

    consume_port_reservation(connection, deployment_id).map_err(|source| {
        CandidateStartError::PortPersistence {
            source,
            resources: Box::new(resources.clone()),
        }
    })?;

    advance_deployment(
        connection,
        deployment_id,
        DeploymentTransition::RuntimeRunning,
    )
    .map_err(|source| CandidateStartError::DeploymentTransition {
        source,
        resources: Box::new(resources.clone()),
    })?;

    Ok(StartedCandidate {
        runtime,
        container_name: name,
        unit_name: unit,
        port: host_port,
    })
}
