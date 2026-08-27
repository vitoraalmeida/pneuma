use std::error::Error;
use std::net::SocketAddr;
use std::path::Path;

use thiserror::Error;

use rusqlite::Connection;

use super::cleanup::CandidateResources;
use super::progress::{DeploymentStep, ProgressReporter};
use super::promotion::{
    PromotePublicCandidateError, begin_public_exposure, promote_public_candidate,
    record_public_exposure_failure,
};
use super::transition::{TransitionDeploymentError, advance_deployment};
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

// Carries the persisted candidate and host paths needed to expose it after internal validation.
pub(crate) struct PublicActivationInput<'a> {
    pub(crate) connection: &'a mut Connection,
    pub(crate) runtime: &'a RuntimeInstance,
    pub(crate) application_id: &'a ApplicationId,
    pub(crate) health_check: &'a HealthCheckSpecification,
    pub(crate) managed_caddy_directory: &'a Path,
    pub(crate) caddyfile_path: &'a Path,
}

// Returns activation data needed by the enclosing deployment finalization.
pub(crate) struct PublicActivationOutput {
    pub(crate) finished_at: String,
}

pub(crate) enum PublicActivationError {
    InternalHealth {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    DeploymentTransition {
        source: TransitionDeploymentError,
        resources: Box<CandidateResources>,
    },
    ExposurePreparation {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    TestGate {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    CaddyMaterialization {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    ExternalHealth {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    PublicPromotion {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
}

// Activates a public candidate in order: internal health, route materialization, external
// health, then persisted promotion; failures retain resources for centralized cleanup.
pub(crate) fn activate_public_candidate(
    input: PublicActivationInput<'_>,
    progress: &mut ProgressReporter<'_>,
) -> Result<PublicActivationOutput, PublicActivationError> {
    let PublicActivationInput {
        connection,
        runtime,
        application_id,
        health_check,
        managed_caddy_directory,
        caddyfile_path,
    } = input;

    let runtime_id = runtime.id.as_str();
    let deployment_id = runtime.deployment_id.as_str();
    let resources =
        CandidateResources::with_container_and_runtime(&runtime.external_runtime_id, &runtime.id);

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

    Ok(PublicActivationOutput {
        finished_at: promoted.finished_at,
    })
}

// Verifies candidate health over its loopback endpoint before any public effect occurs.
fn verify_internal_health(
    runtime_id: &str,
    socket_addr: SocketAddr,
    health_check: &HealthCheckSpecification,
    resources: &CandidateResources,
    progress: &mut ProgressReporter<'_>,
) -> Result<(), PublicActivationError> {
    progress.started(
        DeploymentStep::InternalHealthCheck,
        format!(
            "runtime {runtime_id}, path {}, expected status {}",
            health_check.path().as_str(),
            health_check.expected_status().get()
        ),
    );

    let internal_health = check_internal_health(socket_addr, health_check).map_err(|source| {
        PublicActivationError::InternalHealth {
            source: Box::new(source),
            resources: Box::new(resources.clone()),
        }
    })?;

    if !matches!(internal_health, HealthCheckResult::Healthy { .. }) {
        return Err(PublicActivationError::InternalHealth {
            source: Box::new(PublicHealthFailure {
                result: internal_health,
            }),
            resources: Box::new(resources.clone()),
        });
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
) -> Result<(), PublicActivationError> {
    advance_deployment(connection, deployment_id, DeploymentEvent::Verified).map_err(|source| {
        PublicActivationError::DeploymentTransition {
            source,
            resources: Box::new(resources.clone()),
        }
    })?;

    progress.state_changed(deployment_id.as_str(), DeploymentStatus::Activating);
    wait_for_test_gate("deployment.activating").map_err(|source| {
        PublicActivationError::TestGate {
            source: Box::new(source),
            resources: Box::new(resources.clone()),
        }
    })?;
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
) -> Result<MaterializedPublicRoute, PublicActivationError> {
    let exposure = begin_public_exposure(connection, &runtime.id).map_err(|source| {
        PublicActivationError::ExposurePreparation {
            source: Box::new(source),
            resources: Box::new(resources.clone()),
        }
    })?;

    let endpoint = runtime.expected_endpoint;
    progress.started(
        DeploymentStep::ApplyPublicRoute,
        format!("{} -> {}", exposure.domain, endpoint.socket_addr()),
    );
    let configuration_version =
        ExposureConfigurationVersion::new(&canonical_fragment_contents(&exposure.domain, endpoint))
            .map_err(|source| PublicActivationError::PublicPromotion {
                source: Box::new(source),
                resources: Box::new(resources.clone()),
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

// Translates failed route materialization into its activation error while persisting the
// exposure diagnostic; an unsuccessful adapter rollback upgrades the outcome to divergence.
fn record_materialization_failure(
    connection: &Connection,
    application_id: &ApplicationId,
    source: MaterializeCaddyFragmentError,
    resources: &CandidateResources,
) -> PublicActivationError {
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

    PublicActivationError::CaddyMaterialization {
        source,
        resources: Box::new(resources.clone()),
    }
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
) -> Result<(), PublicActivationError> {
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

        return Err(PublicActivationError::ExternalHealth {
            source,
            resources: Box::new(resources.clone()),
        });
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
) -> Result<PromotedCandidate, PublicActivationError> {
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

            Err(PublicActivationError::PublicPromotion {
                source,
                resources: Box::new(resources.clone()),
            })
        }
    }
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
