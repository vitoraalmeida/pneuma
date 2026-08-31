//! Drives a fully started public candidate to the point of promotion.
//!
//! Owns the public finish variant of the deploy spine: verify internal health,
//! transition to Activating, materialize the Caddy route, verify external health
//! through the public domain, then confirm promotion. Every later-stage failure
//! compensates the prior route state and flows into the shared failure finalizer with
//! the resources it was given — this module never cleans resources itself and does not
//! decide internal promotion (`super::promotion`).

use std::error::Error;
use std::net::SocketAddr;
use std::path::Path;

use thiserror::Error;

use rusqlite::Connection;

use super::cleanup::CandidateResources;
use super::failure::FailedExecution;
use super::progress::{DeploymentStep, ProgressReporter};
use super::promotion::{
    PromotePublicCandidateError, begin_public_exposure, promote_public_candidate,
    record_public_exposure_failure,
};
use super::transition::advance_deployment;
use crate::adapters::caddy_exposure::{
    MaterializeCaddyFragmentError, MaterializedCaddyFragment, canonical_fragment_contents,
    materialize_caddy_fragment, restore_materialized_caddy_fragment,
};
use crate::adapters::health_check_external::check_external_health;
use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::adapters::test_gate::wait_for_test_gate;
use crate::domain::deployment::{
    DeploymentEvent, DeploymentFailureCode, DeploymentStatus, PromotedCandidate,
};
use crate::domain::exposure::{
    DomainName, ExposureConfigurationVersion, ExposureDiagnostic, ExposureOutcome,
};
use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId};
use crate::domain::runtime::{HealthCheckSpecification, RuntimeInstance};

// Carries the persisted candidate, its started unit, and host paths needed to expose it
// after internal validation.
pub(crate) struct PublicActivationInput<'a> {
    pub(crate) connection: &'a mut Connection,
    pub(crate) runtime: &'a RuntimeInstance,
    pub(crate) application_id: &'a ApplicationId,
    pub(crate) health_check: &'a HealthCheckSpecification,
    pub(crate) managed_caddy_directory: &'a Path,
    pub(crate) caddyfile_path: &'a Path,
    pub(crate) unit_name: &'a str,
}

// Activates a public candidate in order: internal health, route materialization, external
// health, then persisted promotion, returning the promoted candidate. Activation runs on a
// fully started candidate, so its container, runtime, unit, and reserved port stay in one
// compensation set, and every failure returns the canonical execution failure with its
// durable code directly.
pub(crate) fn activate_public_candidate(
    input: PublicActivationInput<'_>,
    progress: &mut ProgressReporter<'_>,
) -> Result<PromotedCandidate, FailedExecution> {
    let PublicActivationInput {
        connection,
        runtime,
        application_id,
        health_check,
        managed_caddy_directory,
        caddyfile_path,
        unit_name,
    } = input;

    let runtime_id = runtime.id.as_str();
    let deployment_id = runtime.deployment_id.as_str();
    let resources =
        CandidateResources::with_container_and_runtime(&runtime.external_runtime_id, &runtime.id)
            .with_unit(unit_name)
            .with_port();

    verify_internal_health(
        runtime_id,
        runtime.expected_endpoint.socket_addr(),
        health_check,
        &resources,
        progress,
    )?;
    mark_activating(connection, &runtime.deployment_id, &resources, progress)?;

    let route = materialize_public_route(
        connection,
        application_id,
        runtime,
        managed_caddy_directory,
        caddyfile_path,
        &resources,
        progress,
    )?;
    verify_external_health_or_rollback(
        connection,
        application_id,
        health_check,
        &route,
        caddyfile_path,
        &resources,
        progress,
    )?;

    progress.started(
        DeploymentStep::PromoteCandidate,
        format!("runtime {runtime_id}"),
    );
    let promoted = promote_public_runtime_or_rollback(
        connection,
        application_id,
        &runtime.id,
        &route.configuration_version,
        &route.fragment,
        caddyfile_path,
        &resources,
    )?;
    progress.completed(
        DeploymentStep::PromoteCandidate,
        format!("runtime {runtime_id} promoted to Current"),
    );
    progress.state_changed(deployment_id, DeploymentStatus::Succeeded);

    Ok(promoted)
}

// Verifies candidate health over its loopback endpoint before any public effect occurs.
fn verify_internal_health(
    runtime_id: &str,
    socket_addr: SocketAddr,
    health_check: &HealthCheckSpecification,
    resources: &CandidateResources,
    progress: &mut ProgressReporter<'_>,
) -> Result<(), FailedExecution> {
    progress.started(
        DeploymentStep::InternalHealthCheck,
        format!(
            "runtime {runtime_id}, path {}, expected status {}",
            health_check.path().as_str(),
            health_check.expected_status().get()
        ),
    );

    let internal_health = check_internal_health(socket_addr, health_check).map_err(|source| {
        failed_activation(DeploymentFailureCode::HealthCheck, source, resources)
    })?;

    if !matches!(internal_health, HealthCheckResult::Healthy { .. }) {
        return Err(failed_activation(
            DeploymentFailureCode::HealthCheck,
            PublicHealthFailure {
                result: internal_health,
            },
            resources,
        ));
    }

    progress.completed(
        DeploymentStep::InternalHealthCheck,
        format!("runtime {runtime_id} is healthy"),
    );
    Ok(())
}

// Moves the deployment from Verifying to Activating before any host-visible route work begins.
fn mark_activating(
    connection: &Connection,
    deployment_id: &DeploymentId,
    resources: &CandidateResources,
    progress: &mut ProgressReporter<'_>,
) -> Result<(), FailedExecution> {
    advance_deployment(connection, deployment_id, DeploymentEvent::Verified).map_err(|source| {
        failed_activation(
            DeploymentFailureCode::DeploymentTransition,
            source,
            resources,
        )
    })?;

    progress.state_changed(deployment_id.as_str(), DeploymentStatus::Activating);
    wait_for_test_gate("deployment.activating")
        .map_err(|source| failed_activation(DeploymentFailureCode::TestGate, source, resources))?;
    Ok(())
}

// What later activation phases need from a materialized route: the fragment for rollback,
// the domain for external verification, and the version that promotion must confirm.
struct MaterializedPublicRoute {
    fragment: MaterializedCaddyFragment,
    domain: DomainName,
    configuration_version: ExposureConfigurationVersion,
}

// Prepares the exposure for route work, materializes the managed Caddy fragment, and records
// a route-failure diagnostic so persisted exposure state never silently diverges from the host.
fn materialize_public_route(
    connection: &Connection,
    application_id: &ApplicationId,
    runtime: &RuntimeInstance,
    managed_caddy_directory: &Path,
    caddyfile_path: &Path,
    resources: &CandidateResources,
    progress: &mut ProgressReporter<'_>,
) -> Result<MaterializedPublicRoute, FailedExecution> {
    let exposure = begin_public_exposure(connection, &runtime.id).map_err(|source| {
        failed_activation(
            DeploymentFailureCode::ExposurePreparation,
            source,
            resources,
        )
    })?;

    let endpoint = runtime.expected_endpoint;
    progress.started(
        DeploymentStep::ApplyPublicRoute,
        format!("{} -> {}", exposure.domain, endpoint.socket_addr()),
    );
    let configuration_version =
        ExposureConfigurationVersion::new(&canonical_fragment_contents(&exposure.domain, endpoint))
            .map_err(|source| {
                failed_activation(DeploymentFailureCode::CandidatePromotion, source, resources)
            })?;

    let fragment = materialize_caddy_fragment(
        managed_caddy_directory,
        caddyfile_path,
        application_id,
        &exposure.domain,
        endpoint,
    )
    .map_err(|source| {
        record_materialization_failure(connection, application_id, source, resources)
    })?;
    progress.completed(
        DeploymentStep::ApplyPublicRoute,
        format!("fragment {}", fragment.path.display()),
    );

    Ok(MaterializedPublicRoute {
        fragment,
        domain: exposure.domain,
        configuration_version,
    })
}

// Translates failed route materialization into its canonical failure while persisting the
// exposure diagnostic; an unsuccessful adapter rollback upgrades the outcome to divergence.
fn record_materialization_failure(
    connection: &Connection,
    application_id: &ApplicationId,
    source: MaterializeCaddyFragmentError,
    resources: &CandidateResources,
) -> FailedExecution {
    let outcome = if source.recovery_failed() {
        ExposureOutcome::Diverged
    } else {
        ExposureOutcome::Failed
    };

    let source: Box<dyn Error> = Box::new(source);
    let message = source.to_string();
    let source = record_exposure_failure(
        connection,
        application_id,
        &ExposureDiagnostic::new(
            DeploymentFailureCode::CaddyMaterialization.as_str(),
            &message,
        )
        .expect("static diagnostic code and adapter error messages are valid"),
        outcome,
        source,
    );

    FailedExecution::needing_persistence(
        DeploymentFailureCode::CaddyMaterialization,
        source,
        resources.clone(),
    )
}

// Checks the deployed route through its public domain; on rejection it restores the prior
// route and records the diagnostic before surfacing the external health failure.
fn verify_external_health_or_rollback(
    connection: &Connection,
    application_id: &ApplicationId,
    health_check: &HealthCheckSpecification,
    route: &MaterializedPublicRoute,
    caddyfile_path: &Path,
    resources: &CandidateResources,
    progress: &mut ProgressReporter<'_>,
) -> Result<(), FailedExecution> {
    let MaterializedPublicRoute {
        fragment: materialized,
        domain,
        ..
    } = route;
    progress.started(
        DeploymentStep::ExternalHealthCheck,
        format!("https://{domain}{}", health_check.path().as_str()),
    );

    if let Err(source) =
        check_external_health(domain, health_check.path(), health_check.expected_status())
    {
        let (source, outcome) = rollback_public_route(source, materialized, caddyfile_path);
        let source = record_exposure_failure(
            connection,
            application_id,
            &ExposureDiagnostic::new(
                DeploymentFailureCode::ExternalHealthCheck.as_str(),
                &source.to_string(),
            )
            .expect("static diagnostic code and adapter error messages are valid"),
            outcome,
            source,
        );

        return Err(FailedExecution::needing_persistence(
            DeploymentFailureCode::ExternalHealthCheck,
            source,
            resources.clone(),
        ));
    }

    progress.completed(
        DeploymentStep::ExternalHealthCheck,
        format!("{domain} returned expected status"),
    );
    Ok(())
}

// Confirms the externally verified candidate; when persistence rejects the promotion it
// restores the prior route and records the diagnostic before surfacing the failure.
fn promote_public_runtime_or_rollback(
    connection: &mut Connection,
    application_id: &ApplicationId,
    runtime_id: &RuntimeInstanceId,
    configuration_version: &ExposureConfigurationVersion,
    materialized: &MaterializedCaddyFragment,
    caddyfile_path: &Path,
    resources: &CandidateResources,
) -> Result<PromotedCandidate, FailedExecution> {
    match promote_public_candidate(connection, runtime_id, configuration_version) {
        Ok(promoted) => Ok(promoted),
        Err(source) => {
            let (source, outcome) = rollback_public_route(source, materialized, caddyfile_path);
            let source = record_exposure_failure(
                connection,
                application_id,
                &ExposureDiagnostic::new(
                    DeploymentFailureCode::CandidatePromotion.as_str(),
                    &source.to_string(),
                )
                .expect("static diagnostic code and promotion error messages are valid"),
                outcome,
                source,
            );

            Err(FailedExecution::needing_persistence(
                DeploymentFailureCode::CandidatePromotion,
                source,
                resources.clone(),
            ))
        }
    }
}

// Builds the canonical failure for one activation stage; resources are cloned only on failure.
fn failed_activation(
    code: DeploymentFailureCode,
    source: impl Error + 'static,
    resources: &CandidateResources,
) -> FailedExecution {
    FailedExecution::needing_persistence(code, source, resources.clone())
}

// Records an exposure failure so persisted state matches the host, wrapping persistence
// errors so the original cause is never lost.
fn record_exposure_failure(
    connection: &Connection,
    application_id: &ApplicationId,
    diagnostic: &ExposureDiagnostic,
    outcome: ExposureOutcome,
    original: Box<dyn Error>,
) -> Box<dyn Error> {
    match record_public_exposure_failure(connection, application_id, diagnostic, outcome) {
        Ok(()) => original,
        Err(persistence) => Box::new(ExposureFailureRecordingError {
            original,
            persistence,
        }),
    }
}

// Restores the prior route after activation fails and distinguishes recoverable failure from
// externally diverged Caddy state.
fn rollback_public_route(
    original: impl Error + 'static,
    materialized: &MaterializedCaddyFragment,
    caddyfile_path: &Path,
) -> (Box<dyn Error>, ExposureOutcome) {
    match restore_materialized_caddy_fragment(materialized, caddyfile_path) {
        Ok(()) => (Box::new(original), ExposureOutcome::Failed),
        Err(recovery) => (
            Box::new(PublicRouteRollbackError {
                original: Box::new(original),
                recovery,
            }),
            ExposureOutcome::Diverged,
        ),
    }
}

#[derive(Debug, Error)]
#[error("candidate failed its internal health check: {:?}", result)]
struct PublicHealthFailure {
    result: HealthCheckResult,
}

#[derive(Debug, Error)]
#[error("{}; public route recovery also failed: {}", original, recovery)]
struct PublicRouteRollbackError {
    #[source]
    original: Box<dyn Error>,
    recovery: crate::adapters::caddy_exposure::CaddyRecoveryError,
}

#[derive(Debug, Error)]
#[error(
    "{}; exposure failure could not be recorded: {}",
    original,
    persistence
)]
struct ExposureFailureRecordingError {
    #[source]
    original: Box<dyn Error>,
    persistence: PromotePublicCandidateError,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::path::Path;
    use std::thread;

    use super::*;
    use crate::adapters::database;
    use crate::domain::identity::{ApplicationId, DeploymentId};
    use crate::domain::runtime::{
        ContainerId, ContainerPort, ExpectedRuntimeEndpoint, HealthCheckPath,
        HealthCheckSpecification, HealthCheckStatus, ObservedRuntimeState, RuntimeState,
    };
    use crate::test_support::ScopedExternalPath;

    // The persisted candidate whose started unit and port join every activation failure.
    fn runtime_instance(endpoint: SocketAddr) -> RuntimeInstance {
        RuntimeInstance {
            id: RuntimeInstanceId::new("66666666666666666666666666666666").unwrap(),
            application_id: ApplicationId::new("11111111111111111111111111111111").unwrap(),
            deployment_id: DeploymentId::new("22222222222222222222222222222222").unwrap(),
            external_runtime_id: ContainerId::from("abc123def456"),
            state: RuntimeState::Starting,
            expected_endpoint: ExpectedRuntimeEndpoint::new(endpoint).unwrap(),
            container_port: ContainerPort::new(8080).unwrap(),
            observed_state: ObservedRuntimeState::Running,
            observed_at: "now".to_owned(),
            exit_code: None,
            observation_reason: None,
            retirement: None,
        }
    }

    // A loopback endpoint that answers every internal probe with the expected status.
    fn healthy_endpoint() -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                while !request.ends_with("\r\n\r\n") {
                    if reader.read_line(&mut request).unwrap_or(0) == 0 {
                        break;
                    }
                }
                let _ = reader
                    .get_mut()
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        });
        endpoint
    }

    // A loopback endpoint with no listener, so every internal probe is refused.
    fn dead_endpoint() -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap()
    }

    fn seed_deployment(connection: &Connection, status: &str) {
        connection
            .execute_batch(
                "INSERT INTO systems (id, name) VALUES ('44444444444444444444444444444444', 'team');
                 INSERT INTO applications (
                     id, system_id, name, repository_url, manifest_path, image_repository,
                     container_port, health_check_path, health_check_expected_status, desired_runtime_state
                 ) VALUES (
                     '11111111111111111111111111111111', '44444444444444444444444444444444', 'app',
                     'https://example.test/app.git', 'pneuma.toml', 'registry.example/app',
                     8080, '/healthz', 200, 'stopped');
                 INSERT INTO releases (id, application_id, image_reference, created_at)
                 VALUES (
                     '55555555555555555555555555555555', '11111111111111111111111111111111',
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

    fn run_activation(
        connection: &mut Connection,
        runtime: &RuntimeInstance,
    ) -> Result<(), FailedExecution> {
        let health_check = HealthCheckSpecification::new(
            HealthCheckPath::new("/health").unwrap(),
            HealthCheckStatus::new(200).unwrap(),
        );
        let managed_caddy_directory = std::env::temp_dir().join("pneuma-activation-managed-caddy");
        let caddyfile_path = std::env::temp_dir().join("pneuma-activation-caddyfile");
        activate_public_candidate(
            PublicActivationInput {
                connection,
                runtime,
                application_id: &runtime.application_id,
                health_check: &health_check,
                managed_caddy_directory: &managed_caddy_directory,
                caddyfile_path: &caddyfile_path,
                unit_name: "unit-1",
            },
            &mut ProgressReporter::disabled(),
        )
        .map(|_| ())
    }

    // Activation runs on a fully started candidate, so every failure must carry the whole
    // compensation set: container, runtime, unit, and reserved port.
    fn assert_started_candidate_resources(failed: &FailedExecution) {
        assert!(!failed.failure_persisted());
        let resources = failed.resources();
        assert!(resources.container_id.is_some());
        assert!(resources.runtime_id.is_some());
        assert_eq!(resources.unit_name.as_deref(), Some("unit-1"));
        assert!(resources.port_reserved);
    }

    // Removes the process-global test-gate override even when an assertion fails, because
    // a leaked override would block unrelated gate calls in concurrent tests.
    struct ScopedTestGateDirectory {
        external: ScopedExternalPath,
    }

    impl Drop for ScopedTestGateDirectory {
        fn drop(&mut self) {
            self.external.remove_var("PNEUMA_TEST_GATE_DIRECTORY");
        }
    }

    #[test]
    fn internal_health_failure_returns_the_health_check_code_with_started_resources() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        let runtime = runtime_instance(dead_endpoint());

        let failed = run_activation(&mut connection, &runtime)
            .expect_err("an endpoint with no listener must fail internal health");

        assert_eq!(failed.code(), DeploymentFailureCode::HealthCheck);
        assert_started_candidate_resources(&failed);
    }

    #[test]
    fn deployment_transition_failure_returns_the_transition_code_with_started_resources() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        // The Verified event requires the Verifying stage, so a pending deployment
        // makes the first activation transition fail.
        seed_deployment(&connection, "pending");
        let runtime = runtime_instance(healthy_endpoint());

        let failed = run_activation(&mut connection, &runtime)
            .expect_err("a pending deployment must fail the Verified transition");

        assert_eq!(failed.code(), DeploymentFailureCode::DeploymentTransition);
        assert_started_candidate_resources(&failed);
    }

    #[test]
    fn test_gate_failure_returns_the_test_gate_code_with_started_resources() {
        let external_path = ScopedExternalPath::new("activation-gate", &[]);
        // A regular file where the gate directory should be makes gate setup fail.
        let blocker = external_path.directory().join("gate");
        fs::write(&blocker, "blocker").unwrap();
        external_path.set_var("PNEUMA_TEST_GATE_DIRECTORY", &blocker.to_string_lossy());
        let _gate = ScopedTestGateDirectory {
            external: external_path,
        };

        let mut connection = database::open(Path::new(":memory:")).unwrap();
        seed_deployment(&connection, "verifying");
        let runtime = runtime_instance(healthy_endpoint());

        let failed = run_activation(&mut connection, &runtime)
            .expect_err("an unusable gate directory must fail the activating gate");

        assert_eq!(failed.code(), DeploymentFailureCode::TestGate);
        assert_started_candidate_resources(&failed);
    }

    #[test]
    fn exposure_preparation_failure_returns_the_preparation_code_with_started_resources() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        // The deployment reaches Activating, but the candidate runtime was never
        // persisted, so exposure preparation cannot load its promotion target.
        seed_deployment(&connection, "verifying");
        let runtime = runtime_instance(healthy_endpoint());

        let failed = run_activation(&mut connection, &runtime)
            .expect_err("a missing persisted runtime must fail exposure preparation");

        assert_eq!(failed.code(), DeploymentFailureCode::ExposurePreparation);
        assert_started_candidate_resources(&failed);
    }
}
