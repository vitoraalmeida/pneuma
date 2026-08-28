use std::error::Error;
use std::net::SocketAddr;

use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

use super::cleanup::CandidateResources;
use super::failure::FailedExecution;
use super::transition::advance_deployment;
use crate::adapters::local_runtime::{observe_container, resolve_container_id};
use crate::adapters::port_allocator::{consume_port_reservation, reserve_port};
use crate::adapters::stores::deployment_store::{self, DeploymentStoreError};
use crate::adapters::stores::runtime_store;
use crate::adapters::systemd_quadlet::{container_name, daemon_reload, start, write_unit};
use crate::domain::application::ApplicationName;
use crate::domain::deployment::{DeploymentEvent, DeploymentFailureCode, DeploymentStatus};
use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId};
use crate::domain::release::OciArtifact;
use crate::domain::runtime::{
    ContainerId, ContainerPort, ExpectedRuntimeEndpoint, HostPort, ObservedRuntimeState,
    RuntimeInstance, RuntimeRegistration, RuntimeSpecification,
};

// Returns the observed candidate identity needed by verification and cleanup orchestration.
#[derive(Debug)]
pub(crate) struct StartedCandidate {
    pub(crate) runtime: RuntimeInstance,
    pub(crate) container_name: String,
    pub(crate) unit_name: String,
    pub(crate) port: HostPort,
}

impl StartedCandidate {
    // Tags a failure after full candidate startup so compensation retains every
    // resource a started candidate holds: container, runtime, unit, and reserved port.
    pub(crate) fn failed_execution(
        &self,
        code: DeploymentFailureCode,
        source: impl Error + 'static,
    ) -> FailedExecution {
        FailedExecution::needing_persistence(
            code,
            source,
            CandidateResources::with_container_and_runtime(
                &self.runtime.external_runtime_id,
                &self.runtime.id,
            ),
        )
        .with_started_unit(&self.unit_name)
    }
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
) -> Result<StartedCandidate, FailedExecution> {
    let CandidateStartInput {
        connection,
        deployment_id,
        application_id,
        application_name,
        artifact,
        runtime,
    } = input;

    advance_deployment(connection, deployment_id, DeploymentEvent::Start).map_err(|source| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::DeploymentTransition,
            source,
            CandidateResources::empty(),
        )
    })?;

    let host_port = reserve_port(connection, application_id, deployment_id).map_err(|source| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimePortAllocation,
            source,
            CandidateResources::empty(),
        )
    })?;
    let mut resources = CandidateResources::empty().with_port();

    let unit = write_unit(
        application_name,
        deployment_id,
        artifact,
        runtime.container_port(),
        host_port,
    )
    .map_err(|source| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimeUnitCreation,
            source,
            resources.clone(),
        )
    })?;
    resources = resources.with_unit(&unit);

    daemon_reload().map_err(|source| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimeUnitReload,
            source,
            resources.clone(),
        )
    })?;

    start(&unit).map_err(|source| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimeStart,
            source,
            resources.clone(),
        )
    })?;

    let name = container_name(application_name, deployment_id);
    let container_id = resolve_container_id(&name).map_err(|source| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimeResolution,
            Box::new(source),
            resources.clone(),
        )
    })?;
    resources = resources.with_container_mut(&container_id);

    let observation =
        observe_container(&container_id, runtime.container_port()).map_err(|source| {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::RuntimeObservation,
                Box::new(source),
                resources.clone(),
            )
        })?;

    if *observation.state() != ObservedRuntimeState::Running {
        return Err(FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimeObservation,
            RuntimeObservationFailure::NotRunning {
                actual: observation.state().clone(),
            },
            resources.clone(),
        ));
    }

    let endpoint = observation.observed_endpoint().ok_or_else(|| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimeObservation,
            RuntimeObservationFailure::MissingEndpoint,
            resources.clone(),
        )
    })?;
    let endpoint = ExpectedRuntimeEndpoint::new(endpoint).map_err(|_| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimeObservation,
            RuntimeObservationFailure::InvalidEndpoint,
            resources.clone(),
        )
    })?;

    let runtime = register_candidate_runtime(
        connection,
        deployment_id,
        &container_id,
        endpoint,
        runtime.container_port(),
    )
    .map_err(|source| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimeRegistration,
            Box::new(source),
            resources.clone(),
        )
    })?;
    resources = resources.with_runtime_mut(&runtime.id);

    consume_port_reservation(connection, deployment_id).map_err(|source| {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimePortPersistence,
            source,
            resources.clone(),
        )
    })?;

    advance_deployment(connection, deployment_id, DeploymentEvent::RuntimeRunning).map_err(
        |source| {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::DeploymentTransition,
                source,
                resources.clone(),
            )
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
    Persistence {
        #[source]
        source: Box<dyn Error>,
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
            error => Self::Persistence {
                source: Box::new(error),
            },
        }
    }
}

impl From<rusqlite::Error> for RegisterCandidateRuntimeError {
    fn from(source: rusqlite::Error) -> Self {
        Self::Persistence {
            source: Box::new(source),
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

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) =
        runtime_store::load_runtime_by_external_id(&transaction, external_runtime_id)?
    {
        if matches_existing_registration(&existing, deployment_id, endpoint, container_port) {
            transaction.commit()?;
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
    transaction.commit()?;

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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use rusqlite::Connection;

    use super::*;
    use crate::adapters::database;
    use crate::domain::application::ApplicationName;
    use crate::domain::identity::{ApplicationId, DeploymentId};
    use crate::domain::release::OciArtifact;
    use crate::domain::runtime::{
        ContainerPort, HealthCheckPath, HealthCheckSpecification, HealthCheckStatus,
        RuntimeSpecification,
    };

    // Fake `systemctl`: every invocation is logged; daemon-reload and start fail
    // only when their marker file exists.
    const FAKE_SYSTEMCTL: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_SYSTEMCTL_LOG\"
if [ \"$1\" = \"--user\" ]; then shift; fi
case \"$1\" in
    daemon-reload) if [ -f \"$PNEUMA_FAKE_SYSTEMCTL_RELOAD_FAILURE\" ]; then exit 1; fi ;;
    start) if [ -f \"$PNEUMA_FAKE_SYSTEMCTL_START_FAILURE\" ]; then exit 1; fi ;;
esac
exit 0
";

    // Fake `podman`: identity/state resolution and endpoint observation answer through
    // PNEUMA_FAKE_PODMAN_* variables so every candidate-start stage is controllable.
    const FAKE_PODMAN: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_PODMAN_LOG\"
case \"$1\" in
    inspect)
        case \"$3\" in
            \"{{.Id}}\") printf '%s\\n' \"${PNEUMA_FAKE_PODMAN_ID:-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}\" ;;
            \"{{.State.Status}}\") printf '%s\\n' \"${PNEUMA_FAKE_PODMAN_STATUS:-running}\" ;;
        esac
        exit \"${PNEUMA_FAKE_PODMAN_INSPECT_EXIT:-0}\" ;;
    container)
        exit \"${PNEUMA_FAKE_PODMAN_EXISTS:-0}\" ;;
    port)
        printf '%s\\n' \"${PNEUMA_FAKE_PODMAN_PORT:-127.0.0.1:30000}\" ;;
esac
exit 0
";

    const SYSTEMCTL_BEHAVIOR_VARIABLES: [&str; 2] = [
        "PNEUMA_FAKE_SYSTEMCTL_RELOAD_FAILURE",
        "PNEUMA_FAKE_SYSTEMCTL_START_FAILURE",
    ];

    const PODMAN_BEHAVIOR_VARIABLES: [&str; 2] = [
        "PNEUMA_FAKE_PODMAN_EXISTS",
        "PNEUMA_FAKE_PODMAN_INSPECT_EXIT",
    ];

    // Owns the in-memory database and the faked external boundary for one start_candidate
    // scenario. The process-global environment is serialized behind the shared guards.
    struct CandidateScenario {
        connection: Connection,
        external_path: crate::test_support::ScopedExternalPath,
        quadlet_directory: PathBuf,
        _quadlet_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl CandidateScenario {
        fn new() -> Self {
            let external_path = crate::test_support::ScopedExternalPath::new(
                "candidate-start",
                &[("systemctl", FAKE_SYSTEMCTL), ("podman", FAKE_PODMAN)],
            );
            for variable in SYSTEMCTL_BEHAVIOR_VARIABLES {
                external_path.remove_var(variable);
            }
            for variable in PODMAN_BEHAVIOR_VARIABLES {
                external_path.remove_var(variable);
            }
            let log_directory = external_path.directory().join("logs");
            fs::create_dir_all(&log_directory).unwrap();
            external_path.set_var(
                "PNEUMA_FAKE_SYSTEMCTL_LOG",
                &log_directory.join("systemctl.log").to_string_lossy(),
            );
            external_path.set_var(
                "PNEUMA_FAKE_PODMAN_LOG",
                &log_directory.join("podman.log").to_string_lossy(),
            );
            external_path.set_var("PNEUMA_FAKE_PODMAN_ID", &"a".repeat(64));

            let _quadlet_guard = crate::test_support::lock_quadlet_directory();
            let quadlet_directory = std::env::temp_dir().join(format!(
                "pneuma-candidate-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            // Safety: the guard held in the scenario serializes every PNEUMA_QUADLET_DIR write.
            unsafe { std::env::set_var("PNEUMA_QUADLET_DIR", &quadlet_directory) };

            let connection = database::open(Path::new(":memory:")).unwrap();
            Self {
                connection,
                external_path,
                quadlet_directory,
                _quadlet_guard,
            }
        }
    }

    impl Drop for CandidateScenario {
        fn drop(&mut self) {
            // Safety: _quadlet_guard is still held while this body runs.
            unsafe { std::env::remove_var("PNEUMA_QUADLET_DIR") };
            let _ = fs::remove_dir_all(&self.quadlet_directory);
        }
    }

    fn seed_deployment(connection: &Connection, status: &str) {
        connection
            .execute_batch(
                "INSERT INTO systems (id, name, created_at) VALUES ('44444444444444444444444444444444', 'team', 'now');
                 INSERT INTO applications (
                     id, system_id, name, desired_runtime_state, created_at, updated_at
                 ) VALUES ('11111111111111111111111111111111', '44444444444444444444444444444444', 'app', 'stopped', 'now', 'now');
                 INSERT INTO releases (
                     id, application_id, image_repository, image_digest, image_reference, created_at
                 ) VALUES (
                     '55555555555555555555555555555555', '11111111111111111111111111111111', 'registry.example/app',
                     'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                     'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                     'now'
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO deployments (
                     id, application_id, release_id, type, status, requested_at
                 ) VALUES ('22222222222222222222222222222222', '11111111111111111111111111111111', '55555555555555555555555555555555', 'deploy', ?1, 'now')",
                [status],
            )
            .unwrap();
    }

    fn app_id() -> ApplicationId {
        ApplicationId::new("11111111111111111111111111111111").unwrap()
    }

    fn artifact() -> OciArtifact {
        OciArtifact::parse(
            "registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap()
    }

    fn runtime() -> RuntimeSpecification {
        RuntimeSpecification::new(
            ContainerPort::new(8080).unwrap(),
            HealthCheckSpecification::new(
                HealthCheckPath::new("/health").unwrap(),
                HealthCheckStatus::new(200).unwrap(),
            ),
        )
    }

    fn run_start_candidate(
        scenario: &mut CandidateScenario,
        application_id: &ApplicationId,
    ) -> Result<StartedCandidate, FailedExecution> {
        let deployment_id = DeploymentId::new("22222222222222222222222222222222").unwrap();
        let application_name = ApplicationName::new("app").unwrap();
        let artifact = artifact();
        let runtime = runtime();
        start_candidate(CandidateStartInput {
            connection: &mut scenario.connection,
            deployment_id: &deployment_id,
            application_id,
            application_name: &application_name,
            artifact: &artifact,
            runtime: &runtime,
        })
    }

    // Asserts the exact compensation payload a stage must carry: which resource kinds
    // were already allocated when that stage failed.
    fn assert_resources(
        resources: &CandidateResources,
        port_reserved: bool,
        unit: bool,
        container: bool,
        runtime: bool,
    ) {
        assert_eq!(resources.port_reserved, port_reserved);
        assert_eq!(resources.unit_name.is_some(), unit);
        assert_eq!(resources.container_id.is_some(), container);
        assert_eq!(resources.runtime_id.is_some(), runtime);
    }

    #[test]
    fn start_failure_keeps_transition_code_and_empty_resources_when_the_first_advance_fails() {
        let mut scenario = CandidateScenario::new();

        let failed = run_start_candidate(&mut scenario, &app_id())
            .expect_err("a missing deployment must fail the Start transition");

        assert_eq!(failed.code(), DeploymentFailureCode::DeploymentTransition);
        assert!(!failed.failure_persisted());
        assert_resources(failed.resources(), false, false, false, false);
    }

    #[test]
    fn start_failure_keeps_port_allocation_code_and_empty_resources() {
        let mut scenario = CandidateScenario::new();
        seed_deployment(&scenario.connection, "pending");

        // A nonexistent application makes the reservation insert violate its foreign key.
        let failed = run_start_candidate(
            &mut scenario,
            &ApplicationId::new("33333333333333333333333333333333").unwrap(),
        )
        .expect_err("port reservation must fail for an unknown application");

        assert_eq!(failed.code(), DeploymentFailureCode::RuntimePortAllocation);
        assert!(!failed.failure_persisted());
        assert_resources(failed.resources(), false, false, false, false);
    }

    #[test]
    fn start_failure_keeps_unit_creation_code_and_reserved_port() {
        let mut scenario = CandidateScenario::new();
        seed_deployment(&scenario.connection, "pending");
        // A file blocking the quadlet directory makes unit materialization fail.
        fs::write(&scenario.quadlet_directory, "blocker").unwrap();

        let failed = run_start_candidate(&mut scenario, &app_id())
            .expect_err("unit creation must fail against a blocked directory");

        assert_eq!(failed.code(), DeploymentFailureCode::RuntimeUnitCreation);
        assert!(!failed.failure_persisted());
        assert_resources(failed.resources(), true, false, false, false);
    }

    #[test]
    fn start_failure_keeps_unit_reload_code_and_created_unit() {
        let mut scenario = CandidateScenario::new();
        seed_deployment(&scenario.connection, "pending");
        let marker = scenario.external_path.directory().join("reload-failure");
        fs::write(&marker, "fail").unwrap();
        scenario.external_path.set_var(
            "PNEUMA_FAKE_SYSTEMCTL_RELOAD_FAILURE",
            &marker.to_string_lossy(),
        );

        let failed = run_start_candidate(&mut scenario, &app_id())
            .expect_err("daemon reload must fail against the fake systemctl");

        assert_eq!(failed.code(), DeploymentFailureCode::RuntimeUnitReload);
        assert!(!failed.failure_persisted());
        assert_resources(failed.resources(), true, true, false, false);
    }

    #[test]
    fn start_failure_keeps_unit_start_code_and_created_unit() {
        let mut scenario = CandidateScenario::new();
        seed_deployment(&scenario.connection, "pending");
        let marker = scenario.external_path.directory().join("start-failure");
        fs::write(&marker, "fail").unwrap();
        scenario.external_path.set_var(
            "PNEUMA_FAKE_SYSTEMCTL_START_FAILURE",
            &marker.to_string_lossy(),
        );

        let failed = run_start_candidate(&mut scenario, &app_id())
            .expect_err("unit start must fail against the fake systemctl");

        assert_eq!(failed.code(), DeploymentFailureCode::RuntimeStart);
        assert!(!failed.failure_persisted());
        assert_resources(failed.resources(), true, true, false, false);
    }

    #[test]
    fn start_failure_keeps_resolution_code_and_created_unit() {
        let mut scenario = CandidateScenario::new();
        seed_deployment(&scenario.connection, "pending");
        scenario
            .external_path
            .set_var("PNEUMA_FAKE_PODMAN_INSPECT_EXIT", "1");

        let failed = run_start_candidate(&mut scenario, &app_id())
            .expect_err("container resolution must fail against the fake podman");

        assert_eq!(failed.code(), DeploymentFailureCode::RuntimeResolution);
        assert!(!failed.failure_persisted());
        assert_resources(failed.resources(), true, true, false, false);
    }

    #[test]
    fn start_failure_keeps_observation_code_and_resolved_container() {
        let mut scenario = CandidateScenario::new();
        seed_deployment(&scenario.connection, "pending");
        scenario
            .external_path
            .set_var("PNEUMA_FAKE_PODMAN_EXISTS", "1");

        let failed = run_start_candidate(&mut scenario, &app_id())
            .expect_err("observation must report a missing container");

        assert_eq!(failed.code(), DeploymentFailureCode::RuntimeObservation);
        assert!(!failed.failure_persisted());
        assert_resources(failed.resources(), true, true, true, false);
    }

    #[test]
    fn start_failure_keeps_registration_code_and_resolved_container() {
        let mut scenario = CandidateScenario::new();
        seed_deployment(&scenario.connection, "pending");
        // The fake container ID is already registered to a different endpoint.
        scenario
            .connection
            .execute(
                "INSERT INTO runtime_instances (
                     id, application_id, deployment_id, external_runtime_id, state,
                     host_address, host_port, container_port, last_observed_state,
                     last_observed_at, created_at, updated_at, removed_at
                 ) VALUES ('66666666666666666666666666666666', '11111111111111111111111111111111', '22222222222222222222222222222222', ?1, 'starting',
                           '127.0.0.1', 39999, 8080, 'running',
                           'now', 'now', 'now', NULL)",
                ["a".repeat(64)],
            )
            .unwrap();

        let failed = run_start_candidate(&mut scenario, &app_id())
            .expect_err("a conflicting external runtime id must fail registration");

        assert_eq!(failed.code(), DeploymentFailureCode::RuntimeRegistration);
        assert!(!failed.failure_persisted());
        assert_resources(failed.resources(), true, true, true, false);
    }
}
