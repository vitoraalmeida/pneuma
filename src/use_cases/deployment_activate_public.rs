use std::error::Error;
use std::fmt;
use std::path::Path;

use rusqlite::Connection;

use crate::adapters::caddy_exposure::{
    MaterializedCaddyFragment, canonical_fragment_contents, materialize_caddy_fragment,
    restore_materialized_caddy_fragment,
};
use crate::adapters::health_check_external::check_external_health;
use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::use_cases::deployment_progress::{DeploymentStep, ProgressReporter};
use crate::use_cases::deployment_promote_public::{
    ExposureOutcome, PromotePublicCandidateError, begin_public_exposure, promote_public_candidate,
    record_public_exposure_failure,
};
use crate::use_cases::deployment_register_runtime::CandidateRuntime;
use crate::use_cases::deployment_runtime_cleanup::CandidateResources;
use crate::use_cases::deployment_transition::{DeploymentTransition, advance_deployment};

pub(crate) struct PublicActivationInput<'a> {
    pub connection: &'a mut Connection,
    pub runtime: &'a CandidateRuntime,
    pub application_id: &'a str,
    pub health_path: &'a str,
    pub expected_status: u16,
    pub managed_caddy_directory: &'a Path,
    pub caddyfile_path: &'a Path,
}

pub(crate) struct PublicActivationOutput {
    pub finished_at: String,
}

pub(crate) enum PublicActivationError {
    InternalHealth {
        source: Box<dyn Error>,
        resources: Box<CandidateResources>,
    },
    DeploymentTransition {
        source: crate::use_cases::deployment_transition::TransitionDeploymentError,
        resources: Box<CandidateResources>,
    },
    ExposurePreparation {
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

pub(crate) fn activate_public_candidate(
    input: PublicActivationInput<'_>,
    progress: &mut ProgressReporter<'_>,
) -> Result<PublicActivationOutput, PublicActivationError> {
    let PublicActivationInput {
        connection,
        runtime,
        application_id,
        health_path,
        expected_status,
        managed_caddy_directory,
        caddyfile_path,
    } = input;

    let runtime_id = runtime.id.as_str();
    let container_id = runtime.external_runtime_id.as_str();
    let deployment_id = runtime.deployment_id.as_str();
    let endpoint = runtime.endpoint;

    let resources = CandidateResources::with_container_and_runtime(container_id, runtime_id);

    progress.started(
        DeploymentStep::InternalHealthCheck,
        format!("runtime {runtime_id}, path {health_path}, expected status {expected_status}"),
    );

    let internal_health =
        check_internal_health(endpoint, health_path, expected_status).map_err(|source| {
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
            resources: Box::new(resources),
        });
    }

    progress.completed(
        DeploymentStep::InternalHealthCheck,
        format!("runtime {runtime_id} is healthy"),
    );

    advance_deployment(connection, deployment_id, DeploymentTransition::Verified).map_err(
        |source| PublicActivationError::DeploymentTransition {
            source,
            resources: Box::new(resources.clone()),
        },
    )?;

    progress.state_changed(
        deployment_id,
        crate::domain::deployment::DeploymentStatus::Activating,
    );

    let exposure = begin_public_exposure(connection, runtime_id).map_err(|source| {
        PublicActivationError::ExposurePreparation {
            source: Box::new(source),
            resources: Box::new(resources.clone()),
        }
    })?;

    progress.started(
        DeploymentStep::ApplyPublicRoute,
        format!("{} -> {endpoint}", exposure.domain),
    );
    let configuration_version = canonical_fragment_contents(&exposure.domain, endpoint);

    let materialized = materialize_caddy_fragment(
        managed_caddy_directory,
        caddyfile_path,
        application_id,
        &exposure.domain,
        endpoint,
    )
    .map_err(|source| {
        let outcome = if source.recovery_failed() {
            ExposureOutcome::Diverged
        } else {
            ExposureOutcome::Failed
        };

        let source: Box<dyn Error> = Box::new(source);
        let message = source.to_string();

        let source = match record_public_exposure_failure(
            connection,
            application_id,
            "caddy_materialization_failed",
            &message,
            outcome,
        ) {
            Ok(()) => source,
            Err(persistence) => Box::new(ExposureFailureRecordingError {
                original: source,
                persistence,
            }),
        };

        PublicActivationError::CaddyMaterialization {
            source,
            resources: Box::new(resources.clone()),
        }
    })?;

    progress.completed(
        DeploymentStep::ApplyPublicRoute,
        format!("fragment {}", materialized.path.display()),
    );

    progress.started(
        DeploymentStep::ExternalHealthCheck,
        format!("https://{}{}", exposure.domain, health_path),
    );

    if let Err(source) = check_external_health(&exposure.domain, health_path, expected_status) {
        let (source, outcome) = rollback_public_route(source, &materialized, caddyfile_path);

        let source = match record_public_exposure_failure(
            connection,
            application_id,
            "external_health_check_failed",
            &source.to_string(),
            outcome,
        ) {
            Ok(()) => source,
            Err(persistence) => Box::new(ExposureFailureRecordingError {
                original: source,
                persistence,
            }),
        };

        return Err(PublicActivationError::ExternalHealth {
            source,
            resources: Box::new(resources.clone()),
        });
    }

    progress.completed(
        DeploymentStep::ExternalHealthCheck,
        format!("{} returned expected status", exposure.domain),
    );

    progress.started(
        DeploymentStep::PromoteCandidate,
        format!("runtime {runtime_id}"),
    );

    let promoted = match promote_public_candidate(connection, runtime_id, &configuration_version) {
        Ok(promoted) => promoted,
        Err(source) => {
            let (source, outcome) = rollback_public_route(source, &materialized, caddyfile_path);

            let source = match record_public_exposure_failure(
                connection,
                application_id,
                "candidate_promotion_failed",
                &source.to_string(),
                outcome,
            ) {
                Ok(()) => source,
                Err(persistence) => Box::new(ExposureFailureRecordingError {
                    original: source,
                    persistence,
                }),
            };

            return Err(PublicActivationError::PublicPromotion {
                source,
                resources: Box::new(resources),
            });
        }
    };

    progress.completed(
        DeploymentStep::PromoteCandidate,
        format!("runtime {runtime_id} promoted to Current"),
    );

    progress.state_changed(
        deployment_id,
        crate::domain::deployment::DeploymentStatus::Succeeded,
    );

    Ok(PublicActivationOutput {
        finished_at: promoted.finished_at,
    })
}

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

#[derive(Debug)]
struct PublicHealthFailure {
    result: HealthCheckResult,
}

impl fmt::Display for PublicHealthFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "candidate failed its internal health check: {:?}",
            self.result
        )
    }
}

impl Error for PublicHealthFailure {}

#[derive(Debug)]
struct PublicRouteRollbackError {
    original: Box<dyn Error>,
    recovery: crate::adapters::caddy_exposure::CaddyRecoveryError,
}

impl fmt::Display for PublicRouteRollbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}; public route recovery also failed: {}",
            self.original, self.recovery
        )
    }
}

impl Error for PublicRouteRollbackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.original.as_ref())
    }
}

#[derive(Debug)]
struct ExposureFailureRecordingError {
    original: Box<dyn Error>,
    persistence: PromotePublicCandidateError,
}

impl fmt::Display for ExposureFailureRecordingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}; exposure failure could not be recorded: {}",
            self.original, self.persistence
        )
    }
}

impl Error for ExposureFailureRecordingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.original.as_ref())
    }
}
