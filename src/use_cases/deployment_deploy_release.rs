use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::adapters::caddy_exposure::{
    materialize_caddy_fragment, restore_materialized_caddy_fragment,
};
use crate::adapters::health_check_external::check_external_health;
use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::manifest::Visibility;
use crate::domain::release::Release;
use crate::use_cases::deployment_create::{
    CreateDeploymentError, DeploymentStatus, DeploymentType, create_deployment,
};
use crate::use_cases::deployment_progress::{DeploymentProgress, DeploymentStep, ProgressReporter};
use crate::use_cases::deployment_promote_internal::{
    PromoteInternalCandidateError, promote_internal_candidate,
};
use crate::use_cases::deployment_promote_public::{
    ExposureOutcome, PromotePublicCandidateError, begin_public_exposure, promote_public_candidate,
    record_public_exposure_failure,
};
use crate::use_cases::deployment_register_runtime::CandidateRuntime;
use crate::use_cases::deployment_runtime_cleanup::{
    CandidateCleanupError, CandidateResources, cleanup_failed_candidate, load_previous_runtime,
    retire_previous_runtime,
};
use crate::use_cases::deployment_start_candidate::{
    CandidateStartError, CandidateStartInput, start_candidate,
};
use crate::use_cases::deployment_transition::{
    DeploymentTransition, TransitionDeploymentError, advance_deployment, fail_deployment,
};

#[derive(Debug, PartialEq, Eq)]
pub struct DeployedRelease {
    pub deployment_id: String,
    pub runtime_id: String,
    pub container_name: String,
    pub image_reference: String,
    pub source_revision: Option<String>,
    pub finished_at: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PublicDeploymentConfiguration {
    pub managed_caddy_directory: PathBuf,
    pub caddyfile_path: PathBuf,
}

#[derive(Debug)]
pub enum DeployReleaseError {
    ApplicationNotFound {
        application_id: String,
    },
    PublicApplication {
        application_id: String,
    },
    LoadApplication {
        source: rusqlite::Error,
    },
    CreateDeployment {
        source: CreateDeploymentError,
    },
    DeploymentFailed {
        deployment_id: String,
        code: &'static str,
        source: Box<dyn Error>,
    },
    RecordFailure {
        deployment_id: String,
        failure: String,
        source: TransitionDeploymentError,
    },
    Cleanup {
        deployment_id: String,
        failure: String,
        source: Box<CandidateCleanupError>,
    },
}

impl fmt::Display for DeployReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::PublicApplication { application_id } => write!(
                formatter,
                "application `{application_id}` requires public deployment support"
            ),
            Self::LoadApplication { source } => {
                write!(
                    formatter,
                    "failed to load deployment specification: {source}"
                )
            }
            Self::CreateDeployment { source } => write!(formatter, "{source}"),
            Self::DeploymentFailed {
                deployment_id,
                code,
                source,
            } => write!(
                formatter,
                "deployment `{deployment_id}` failed with `{code}`: {source}"
            ),
            Self::RecordFailure {
                deployment_id,
                failure,
                source,
            } => write!(
                formatter,
                "deployment `{deployment_id}` encountered `{failure}` and its failure could not be recorded: {source}"
            ),
            Self::Cleanup {
                deployment_id,
                failure,
                source,
            } => write!(
                formatter,
                "deployment `{deployment_id}` encountered `{failure}` and its candidate could not be cleaned up: {source}"
            ),
        }
    }
}

impl Error for DeployReleaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::LoadApplication { source } => Some(source),
            Self::CreateDeployment { source } => Some(source),
            Self::DeploymentFailed { source, .. } => Some(source.as_ref()),
            Self::RecordFailure { source, .. } => Some(source),
            Self::Cleanup { source, .. } => Some(source.as_ref()),
            Self::ApplicationNotFound { .. } | Self::PublicApplication { .. } => None,
        }
    }
}

pub fn deploy_release(
    connection: &mut Connection,
    application_id: &str,
    release: &Release,
    deployment_type: DeploymentType,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeployedRelease, DeployReleaseError> {
    let mut progress = ProgressReporter::disabled();
    deploy_release_reporting(
        connection,
        application_id,
        release,
        deployment_type,
        public_configuration,
        &mut progress,
    )
}

pub fn deploy_release_with_progress(
    connection: &mut Connection,
    application_id: &str,
    release: &Release,
    deployment_type: DeploymentType,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut dyn FnMut(DeploymentProgress),
) -> Result<DeployedRelease, DeployReleaseError> {
    let mut progress = ProgressReporter::enabled(progress);
    deploy_release_reporting(
        connection,
        application_id,
        release,
        deployment_type,
        public_configuration,
        &mut progress,
    )
}

fn deploy_release_reporting(
    connection: &mut Connection,
    application_id: &str,
    release: &Release,
    deployment_type: DeploymentType,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeployedRelease, DeployReleaseError> {
    progress.started(
        DeploymentStep::LoadSpecification,
        format!("application {application_id}"),
    );
    let specification = load_specification(connection, application_id)?;
    progress.completed(
        DeploymentStep::LoadSpecification,
        format!(
            "application {}, visibility {}",
            specification.application_name,
            specification.visibility.database_value()
        ),
    );
    if specification.visibility == Visibility::Public && public_configuration.is_none() {
        return Err(DeployReleaseError::PublicApplication {
            application_id: application_id.to_owned(),
        });
    }

    progress.started(
        DeploymentStep::CreateDeployment,
        format!("release {}", release.id),
    );
    let deployment = create_deployment(connection, application_id, &release.id, deployment_type)
        .map_err(|source| DeployReleaseError::CreateDeployment { source })?;
    progress.completed(
        DeploymentStep::CreateDeployment,
        format!("deployment {}", deployment.id),
    );
    progress.state_changed(&deployment.id, DeploymentStatus::Pending);

    let runtime_identity = release.source_revision.as_deref().unwrap_or(&release.id);
    let execution = execute_deployment(
        connection,
        &deployment.id,
        &specification,
        &release.image_reference,
        runtime_identity,
        public_configuration,
        progress,
    );
    match execution {
        Ok((runtime_id, container_name, finished_at)) => Ok(DeployedRelease {
            deployment_id: deployment.id,
            runtime_id,
            container_name,
            image_reference: release.image_reference.clone(),
            source_revision: release.source_revision.clone(),
            finished_at,
        }),
        Err(failed) => finish_failed_deployment(connection, &deployment.id, failed, progress),
    }
}

struct DeploymentSpecification {
    application_id: String,
    application_name: String,
    container_port: u16,
    health_path: String,
    expected_status: u16,
    visibility: Visibility,
}

fn load_specification(
    connection: &Connection,
    application_id: &str,
) -> Result<DeploymentSpecification, DeployReleaseError> {
    let spec = match application_store::load_deployment_specification(connection, application_id) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return Err(DeployReleaseError::ApplicationNotFound {
                application_id: application_id.to_owned(),
            });
        }
        Err(ApplicationStoreError::NotFound { application_id }) => {
            return Err(DeployReleaseError::ApplicationNotFound { application_id });
        }
        Err(ApplicationStoreError::SystemNotFound { .. }) => {
            return Err(DeployReleaseError::LoadApplication {
                source: rusqlite::Error::QueryReturnedNoRows,
            });
        }
        Err(ApplicationStoreError::Persistence { source }) => {
            return Err(DeployReleaseError::LoadApplication { source });
        }
    };

    let visibility =
        Visibility::from_database(&spec.5).ok_or_else(|| DeployReleaseError::LoadApplication {
            source: rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid visibility: {}", spec.5),
                )),
            ),
        })?;

    Ok(DeploymentSpecification {
        application_id: spec.0,
        application_name: spec.1,
        container_port: spec.2,
        health_path: spec.3,
        expected_status: spec.4,
        visibility,
    })
}

struct FailedExecution {
    code: &'static str,
    source: Box<dyn Error>,
    failure_persisted: bool,
    resources: CandidateResources,
}

fn execute_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    specification: &DeploymentSpecification,
    image_reference: &str,
    source_revision: &str,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<(String, String, String), FailedExecution> {
    progress.state_changed(deployment_id, DeploymentStatus::Starting);
    progress.started(
        DeploymentStep::CreateContainer,
        format!("image {image_reference}"),
    );

    let input = CandidateStartInput {
        connection,
        deployment_id,
        application_id: &specification.application_id,
        application_name: &specification.application_name,
        image_reference,
        container_port: specification.container_port,
        source_revision,
    };

    let candidate = start_candidate(input).map_err(|err| match err {
        CandidateStartError::PortAllocation { source } => {
            failure_needing_persistence("runtime_port_allocation_failed", source, None, None)
        }
        CandidateStartError::UnitCreation { source, resources } => FailedExecution {
            code: "runtime_unit_creation_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::UnitReload { source, resources } => FailedExecution {
            code: "runtime_unit_reload_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::UnitStart { source, resources } => FailedExecution {
            code: "runtime_start_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::ContainerResolution { source, resources } => FailedExecution {
            code: "runtime_resolution_failed",
            source,
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::ContainerObservation { source, resources } => FailedExecution {
            code: "runtime_observation_failed",
            source,
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::RuntimeRegistration { source, resources } => FailedExecution {
            code: "runtime_registration_failed",
            source,
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::PortPersistence { source, resources } => FailedExecution {
            code: "runtime_port_persistence_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::DeploymentTransition { source, resources } => FailedExecution {
            code: "deployment_transition_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
    })?;

    progress.completed(
        DeploymentStep::CreateContainer,
        format!(
            "unit {}, endpoint 127.0.0.1:{}",
            candidate.unit_name, candidate.port
        ),
    );
    progress.completed(
        DeploymentStep::StartContainer,
        format!("container {}", candidate.runtime.external_runtime_id),
    );
    progress.completed(
        DeploymentStep::ObserveContainer,
        format!("state Running, endpoint {}", candidate.runtime.endpoint),
    );
    progress.completed(
        DeploymentStep::RegisterCandidate,
        format!("runtime {}", candidate.runtime.id),
    );
    progress.state_changed(deployment_id, DeploymentStatus::Verifying);

    let previous_runtime = load_previous_runtime(
        connection,
        &specification.application_id,
        &candidate.runtime.id,
    )
    .map_err(|source| {
        candidate_failure(
            "runtime_reconciliation_failed",
            source,
            Some(&candidate.runtime.external_runtime_id),
            Some(&candidate.runtime.id),
            Some(&candidate.unit_name),
            true,
        )
    })?;

    if specification.visibility == Visibility::Public {
        let Some(public_configuration) = public_configuration else {
            return Err(failure_needing_persistence(
                "public_configuration_missing",
                DeployReleaseError::PublicApplication {
                    application_id: specification.application_id.clone(),
                },
                Some(&candidate.runtime.external_runtime_id),
                Some(&candidate.runtime.id),
            ));
        };
        let completed = execute_public_candidate(
            connection,
            specification,
            &candidate.runtime,
            source_revision,
            public_configuration,
            progress,
        );
        if completed.is_ok() {
            retire_previous_runtime(
                connection,
                &specification.application_name,
                previous_runtime.as_ref(),
            );
        }
        return completed
            .map(|(runtime_id, finished_at)| (runtime_id, candidate.container_name, finished_at));
    }

    progress.started(
        DeploymentStep::HealthCheckAndPromotion,
        format!(
            "runtime {}, path {}, expected status {}",
            candidate.runtime.id, specification.health_path, specification.expected_status
        ),
    );
    let promoted = promote_internal_candidate(
        connection,
        &candidate.runtime.id,
        &specification.health_path,
        specification.expected_status,
    )
    .map_err(|source| {
        if matches!(
            &source,
            PromoteInternalCandidateError::CandidateUnhealthy { .. }
        ) {
            failure_already_persisted(
                "health_check_failed",
                source,
                &candidate.runtime.external_runtime_id,
                &candidate.runtime.id,
            )
        } else {
            failure_needing_persistence(
                "candidate_promotion_failed",
                source,
                Some(&candidate.runtime.external_runtime_id),
                Some(&candidate.runtime.id),
            )
        }
    })?;
    progress.completed(
        DeploymentStep::HealthCheckAndPromotion,
        format!("runtime {} promoted to Current", candidate.runtime.id),
    );
    progress.state_changed(deployment_id, DeploymentStatus::Succeeded);
    retire_previous_runtime(
        connection,
        &specification.application_name,
        previous_runtime.as_ref(),
    );

    Ok((
        candidate.runtime.id,
        candidate.container_name,
        promoted.finished_at,
    ))
}

fn execute_public_candidate(
    connection: &mut Connection,
    specification: &DeploymentSpecification,
    runtime: &CandidateRuntime,
    commit_sha: &str,
    public_configuration: &PublicDeploymentConfiguration,
    progress: &mut ProgressReporter<'_>,
) -> Result<(String, String), FailedExecution> {
    let runtime_id = runtime.id.as_str();
    let container_id = runtime.external_runtime_id.as_str();
    let deployment_id = runtime.deployment_id.as_str();
    let endpoint = runtime.endpoint;
    progress.started(
        DeploymentStep::InternalHealthCheck,
        format!(
            "runtime {runtime_id}, path {}, expected status {}",
            specification.health_path, specification.expected_status
        ),
    );
    let internal_health = check_internal_health(
        endpoint,
        &specification.health_path,
        specification.expected_status,
    )
    .map_err(|source| {
        failure_needing_persistence(
            "health_check_failed",
            source,
            Some(container_id),
            Some(runtime_id),
        )
    })?;
    if !matches!(internal_health, HealthCheckResult::Healthy { .. }) {
        return Err(failure_needing_persistence(
            "health_check_failed",
            PublicHealthFailure {
                result: internal_health,
            },
            Some(container_id),
            Some(runtime_id),
        ));
    }
    progress.completed(
        DeploymentStep::InternalHealthCheck,
        format!("runtime {runtime_id} is healthy"),
    );
    advance_deployment(connection, deployment_id, DeploymentTransition::Verified).map_err(
        |source| {
            failure_needing_persistence(
                "deployment_transition_failed",
                source,
                Some(container_id),
                Some(runtime_id),
            )
        },
    )?;
    progress.state_changed(deployment_id, DeploymentStatus::Activating);

    let exposure = begin_public_exposure(connection, runtime_id).map_err(|source| {
        failure_needing_persistence(
            "exposure_preparation_failed",
            source,
            Some(container_id),
            Some(runtime_id),
        )
    })?;
    progress.started(
        DeploymentStep::ApplyPublicRoute,
        format!("{} -> {endpoint}", exposure.domain),
    );
    let materialized = materialize_caddy_fragment(
        &public_configuration.managed_caddy_directory,
        &public_configuration.caddyfile_path,
        &specification.application_id,
        &exposure.domain,
        endpoint,
    )
    .map_err(|source| {
        let outcome = if source.recovery_failed() {
            ExposureOutcome::Diverged
        } else {
            ExposureOutcome::Failed
        };
        public_failure(
            connection,
            &exposure.application_id,
            "caddy_materialization_failed",
            Box::new(source),
            container_id,
            runtime_id,
            outcome,
        )
    })?;
    progress.completed(
        DeploymentStep::ApplyPublicRoute,
        format!("fragment {}", materialized.path.display()),
    );
    progress.started(
        DeploymentStep::ExternalHealthCheck,
        format!("https://{}{}", exposure.domain, specification.health_path),
    );
    if let Err(source) = check_external_health(
        &exposure.domain,
        &specification.health_path,
        specification.expected_status,
    ) {
        let (source, outcome) =
            rollback_public_route(source, &materialized, &public_configuration.caddyfile_path);
        return Err(public_failure(
            connection,
            &exposure.application_id,
            "external_health_check_failed",
            source,
            container_id,
            runtime_id,
            outcome,
        ));
    }
    progress.completed(
        DeploymentStep::ExternalHealthCheck,
        format!("{} returned expected status", exposure.domain),
    );

    progress.started(
        DeploymentStep::PromoteCandidate,
        format!("runtime {runtime_id}"),
    );
    let promoted = match promote_public_candidate(connection, runtime_id, commit_sha) {
        Ok(promoted) => promoted,
        Err(source) => {
            let (source, outcome) =
                rollback_public_route(source, &materialized, &public_configuration.caddyfile_path);
            return Err(public_failure(
                connection,
                &exposure.application_id,
                "candidate_promotion_failed",
                source,
                container_id,
                runtime_id,
                outcome,
            ));
        }
    };
    progress.completed(
        DeploymentStep::PromoteCandidate,
        format!("runtime {runtime_id} promoted to Current"),
    );
    progress.state_changed(deployment_id, DeploymentStatus::Succeeded);

    Ok((runtime_id.to_owned(), promoted.finished_at))
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

fn rollback_public_route(
    original: impl Error + 'static,
    materialized: &crate::adapters::caddy_exposure::MaterializedCaddyFragment,
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

fn public_failure(
    connection: &Connection,
    application_id: &str,
    code: &'static str,
    source: Box<dyn Error>,
    container_id: &str,
    runtime_id: &str,
    outcome: ExposureOutcome,
) -> FailedExecution {
    let message = source.to_string();
    let source =
        match record_public_exposure_failure(connection, application_id, code, &message, outcome) {
            Ok(()) => source,
            Err(persistence) => Box::new(ExposureFailureRecordingError {
                original: source,
                persistence,
            }),
        };
    FailedExecution {
        code,
        source,
        failure_persisted: false,
        resources: CandidateResources::with_container_and_runtime(container_id, runtime_id),
    }
}

fn finish_failed_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    failed: FailedExecution,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeployedRelease, DeployReleaseError> {
    let failure = failed.source.to_string();
    let record_error = if failed.failure_persisted {
        progress.failure_persisted(deployment_id, failed.code);
        None
    } else {
        match fail_deployment(connection, deployment_id, failed.code, &failure) {
            Ok(_) => {
                progress.failure_persisted(deployment_id, failed.code);
                None
            }
            Err(source) => Some(source),
        }
    };
    let cleanup_error = if failed.resources.container_id.is_some()
        || failed.resources.unit_name.is_some()
        || failed.resources.port_reserved
    {
        progress.started(
            DeploymentStep::CleanupCandidate,
            format!("deployment {deployment_id}"),
        );
        match cleanup_failed_candidate(
            connection,
            deployment_id,
            failed.resources.unit_name.as_deref(),
            failed.resources.container_id.as_deref(),
            failed.resources.runtime_id.as_deref(),
        ) {
            Ok(()) => {
                progress.completed(
                    DeploymentStep::CleanupCandidate,
                    format!("deployment {deployment_id}"),
                );
                None
            }
            Err(source) => Some(source),
        }
    } else {
        None
    };

    if let Some(source) = cleanup_error {
        return Err(DeployReleaseError::Cleanup {
            deployment_id: deployment_id.to_owned(),
            failure,
            source: Box::new(source),
        });
    }
    if let Some(source) = record_error {
        return Err(DeployReleaseError::RecordFailure {
            deployment_id: deployment_id.to_owned(),
            failure,
            source,
        });
    }

    Err(DeployReleaseError::DeploymentFailed {
        deployment_id: deployment_id.to_owned(),
        code: failed.code,
        source: failed.source,
    })
}

// Git, build, runtime, and ordinary promotion errors do not update the deployment
// themselves. Tag them as needing persistence so the common finalizer records the
// correct failure stage before performing any candidate cleanup.
fn failure_needing_persistence(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
) -> FailedExecution {
    let resources = match (container_id, runtime_id) {
        (Some(cid), Some(rid)) => CandidateResources::with_container_and_runtime(cid, rid),
        (Some(cid), None) => CandidateResources::with_container(cid),
        _ => CandidateResources::empty(),
    };
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: false,
        resources,
    }
}

fn candidate_failure(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
    unit_name: Option<&str>,
    port_reserved: bool,
) -> FailedExecution {
    let mut resources = match (container_id, runtime_id) {
        (Some(cid), Some(rid)) => CandidateResources::with_container_and_runtime(cid, rid),
        (Some(cid), None) => CandidateResources::with_container(cid),
        _ => CandidateResources::empty(),
    };
    if let Some(unit) = unit_name {
        resources = resources.with_unit(unit);
    }
    if port_reserved {
        resources = resources.with_port();
    }
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: false,
        resources,
    }
}

// An unhealthy-candidate result is different: promotion persists `Failed` before it
// returns the error. Tag it as already persisted so the finalizer removes the rejected
// candidate without trying to fail an already-terminal deployment a second time.
fn failure_already_persisted(
    code: &'static str,
    source: impl Error + 'static,
    container_id: &str,
    runtime_id: &str,
) -> FailedExecution {
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: true,
        resources: CandidateResources::with_container_and_runtime(container_id, runtime_id),
    }
}
