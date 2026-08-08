use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::adapters::caddy_exposure::{
    materialize_caddy_fragment, restore_materialized_caddy_fragment,
};
use crate::adapters::external_health::check_external_health;
use crate::adapters::git_source::ResolveCommitError;
use crate::adapters::health_check::{HealthCheckError, HealthCheckResult, check_internal_health};
use crate::adapters::local_runtime::{
    ControlContainerError, ObserveContainerError, ObservedRuntimeState, observe_container,
    remove_container, resolve_container_id,
};
use crate::adapters::port_allocator::{
    PortAllocationError, consume_port_reservation, release_port, reserve_port,
};
use crate::adapters::systemd_quadlet::{
    QuadletError, container_name, daemon_reload, disable, enable, remove_unit, start, stop,
    unit_name, write_unit,
};
use crate::domain::manifest::Visibility;
use crate::domain::release::Release;
use crate::use_cases::deployment_create::{
    CreateDeploymentError, DeploymentStatus, DeploymentType, create_deployment,
};
use crate::use_cases::deployment_promote_internal::{
    PromoteInternalCandidateError, promote_internal_candidate,
};
use crate::use_cases::deployment_promote_public::{
    ExposureOutcome, PromotePublicCandidateError, begin_public_exposure, promote_public_candidate,
    record_public_exposure_failure,
};
use crate::use_cases::deployment_register_runtime::{CandidateRuntime, register_candidate_runtime};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentStep {
    LoadSpecification,
    ResolveRevision,
    ReconcileRuntime,
    CreateDeployment,
    PrepareCheckout,
    BuildImage,
    CreateContainer,
    StartContainer,
    ObserveContainer,
    RegisterCandidate,
    HealthCheckAndPromotion,
    InternalHealthCheck,
    ApplyPublicRoute,
    ExternalHealthCheck,
    PromoteCandidate,
    CleanupCandidate,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeploymentProgress {
    StepStarted {
        step: DeploymentStep,
        detail: String,
    },
    StepCompleted {
        step: DeploymentStep,
        detail: String,
    },
    StateChanged {
        deployment_id: String,
        status: DeploymentStatus,
    },
    FailurePersisted {
        deployment_id: String,
        code: &'static str,
    },
}

impl fmt::Display for DeploymentStep {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::LoadSpecification => "load application specification",
            Self::ResolveRevision => "resolve Git revision",
            Self::ReconcileRuntime => "reconcile existing runtime",
            Self::CreateDeployment => "create deployment",
            Self::PrepareCheckout => "prepare checkout",
            Self::BuildImage => "build image",
            Self::CreateContainer => "create candidate container",
            Self::StartContainer => "start candidate container",
            Self::ObserveContainer => "observe candidate container",
            Self::RegisterCandidate => "register candidate runtime",
            Self::HealthCheckAndPromotion => "health check and promotion",
            Self::InternalHealthCheck => "internal health check",
            Self::ApplyPublicRoute => "apply public route",
            Self::ExternalHealthCheck => "external health check",
            Self::PromoteCandidate => "promote public candidate",
            Self::CleanupCandidate => "clean up candidate",
        };
        formatter.write_str(name)
    }
}

impl fmt::Display for DeploymentProgress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StepStarted { step, detail } => {
                write!(formatter, "{step}: started ({detail})")
            }
            Self::StepCompleted { step, detail } => {
                write!(formatter, "{step}: completed ({detail})")
            }
            Self::StateChanged {
                deployment_id,
                status,
            } => write!(
                formatter,
                "deployment {deployment_id}: state changed to {status:?}"
            ),
            Self::FailurePersisted {
                deployment_id,
                code,
            } => write!(
                formatter,
                "deployment {deployment_id}: state changed to Failed; failure persisted ({code})"
            ),
        }
    }
}

struct ProgressReporter<'a> {
    callback: Option<&'a mut dyn FnMut(DeploymentProgress)>,
}

impl<'a> ProgressReporter<'a> {
    fn disabled() -> Self {
        Self { callback: None }
    }

    fn enabled(callback: &'a mut dyn FnMut(DeploymentProgress)) -> Self {
        Self {
            callback: Some(callback),
        }
    }

    fn started(&mut self, step: DeploymentStep, detail: String) {
        self.emit(DeploymentProgress::StepStarted { step, detail });
    }

    fn completed(&mut self, step: DeploymentStep, detail: String) {
        self.emit(DeploymentProgress::StepCompleted { step, detail });
    }

    fn state_changed(&mut self, deployment_id: &str, status: DeploymentStatus) {
        self.emit(DeploymentProgress::StateChanged {
            deployment_id: deployment_id.to_owned(),
            status,
        });
    }

    fn failure_persisted(&mut self, deployment_id: &str, code: &'static str) {
        self.emit(DeploymentProgress::FailurePersisted {
            deployment_id: deployment_id.to_owned(),
            code,
        });
    }

    fn emit(&mut self, event: DeploymentProgress) {
        if let Some(callback) = &mut self.callback {
            callback(event);
        }
    }
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
    ResolveRevision {
        source: ResolveCommitError,
    },
    LoadExistingRuntime {
        source: rusqlite::Error,
    },
    ObserveExistingRuntime {
        runtime_id: String,
        source: ObserveContainerError,
    },
    StartExistingRuntime {
        runtime_id: String,
        source: ControlContainerError,
    },
    PersistExistingRuntime {
        runtime_id: String,
        source: rusqlite::Error,
    },
    ExistingRuntimeChanged {
        runtime_id: String,
    },
    ExistingRuntimeUnavailable {
        runtime_id: String,
        state: ObservedRuntimeState,
    },
    CheckExistingRuntime {
        runtime_id: String,
        source: HealthCheckError,
    },
    ExistingRuntimeUnhealthy {
        runtime_id: String,
        result: HealthCheckResult,
    },
    ExistingPublicRouteMismatch {
        runtime_id: String,
    },
    ReactivatePreviousRuntime {
        runtime_id: String,
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

#[derive(Debug)]
pub enum CandidateCleanupError {
    StopUnit { source: QuadletError },
    RemoveUnit { source: QuadletError },
    ReloadUnits { source: QuadletError },
    RemoveContainer { source: ControlContainerError },
    ReleasePort { source: PortAllocationError },
    Persistence { source: rusqlite::Error },
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
            Self::ResolveRevision { source } => write!(formatter, "{source}"),
            Self::LoadExistingRuntime { source } => {
                write!(formatter, "failed to load the existing runtime: {source}")
            }
            Self::ObserveExistingRuntime { runtime_id, source } => write!(
                formatter,
                "failed to observe existing runtime `{runtime_id}`: {source}"
            ),
            Self::StartExistingRuntime { runtime_id, source } => write!(
                formatter,
                "failed to restart existing runtime `{runtime_id}`: {source}"
            ),
            Self::PersistExistingRuntime { runtime_id, source } => write!(
                formatter,
                "failed to persist observation of existing runtime `{runtime_id}`: {source}"
            ),
            Self::ExistingRuntimeChanged { runtime_id } => write!(
                formatter,
                "existing runtime `{runtime_id}` changed while it was being reconciled"
            ),
            Self::ExistingRuntimeUnavailable { runtime_id, state } => write!(
                formatter,
                "existing runtime `{runtime_id}` cannot be reconciled from state {state:?}"
            ),
            Self::CheckExistingRuntime { runtime_id, source } => write!(
                formatter,
                "failed to check existing runtime `{runtime_id}` health: {source}"
            ),
            Self::ExistingRuntimeUnhealthy { runtime_id, result } => write!(
                formatter,
                "existing runtime `{runtime_id}` is unhealthy: {result:?}"
            ),
            Self::ExistingPublicRouteMismatch { runtime_id } => write!(
                formatter,
                "public runtime `{runtime_id}` is healthy but is not the active materialized route"
            ),
            Self::ReactivatePreviousRuntime { runtime_id, source } => write!(
                formatter,
                "failed to reactivate previous runtime `{runtime_id}`: {source}"
            ),
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
            Self::ResolveRevision { source } => Some(source),
            Self::LoadExistingRuntime { source } => Some(source),
            Self::ObserveExistingRuntime { source, .. } => Some(source),
            Self::StartExistingRuntime { source, .. } => Some(source),
            Self::PersistExistingRuntime { source, .. } => Some(source),
            Self::CheckExistingRuntime { source, .. } => Some(source),
            Self::ReactivatePreviousRuntime { source, .. } => Some(source),
            Self::CreateDeployment { source } => Some(source),
            Self::DeploymentFailed { source, .. } => Some(source.as_ref()),
            Self::RecordFailure { source, .. } => Some(source),
            Self::Cleanup { source, .. } => Some(source.as_ref()),
            Self::ApplicationNotFound { .. }
            | Self::PublicApplication { .. }
            | Self::ExistingRuntimeChanged { .. }
            | Self::ExistingRuntimeUnavailable { .. }
            | Self::ExistingRuntimeUnhealthy { .. }
            | Self::ExistingPublicRouteMismatch { .. } => None,
        }
    }
}

impl fmt::Display for CandidateCleanupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RemoveContainer { source } => write!(formatter, "{source}"),
            Self::StopUnit { source }
            | Self::RemoveUnit { source }
            | Self::ReloadUnits { source } => write!(formatter, "{source}"),
            Self::ReleasePort { source } => write!(formatter, "{source}"),
            Self::Persistence { source } => {
                write!(formatter, "failed to persist candidate removal: {source}")
            }
        }
    }
}

impl Error for CandidateCleanupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RemoveContainer { source } => Some(source),
            Self::StopUnit { source }
            | Self::RemoveUnit { source }
            | Self::ReloadUnits { source } => Some(source),
            Self::ReleasePort { source } => Some(source),
            Self::Persistence { source } => Some(source),
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
    connection
        .query_row(
            "SELECT
                applications.id,
                applications.name,
                application_build_specs.containerfile_path,
                application_build_specs.context_path,
                application_runtime_specs.container_port,
                health_check_specs.path,
                health_check_specs.expected_status,
                exposures.desired_visibility
             FROM applications
             JOIN application_build_specs
                ON application_build_specs.application_id = applications.id
             JOIN application_runtime_specs
                ON application_runtime_specs.application_id = applications.id
             JOIN health_check_specs
                ON health_check_specs.application_id = applications.id
             JOIN exposures ON exposures.application_id = applications.id
             WHERE applications.id = ?1",
            [application_id],
            |row| {
                let visibility_text: String = row.get(7)?;
                let visibility = Visibility::from_database(&visibility_text).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("invalid visibility: {visibility_text}"),
                        )),
                    )
                })?;
                Ok(DeploymentSpecification {
                    application_id: row.get(0)?,
                    application_name: row.get(1)?,
                    container_port: row.get(4)?,
                    health_path: row.get(5)?,
                    expected_status: row.get(6)?,
                    visibility,
                })
            },
        )
        .optional()
        .map_err(|source| DeployReleaseError::LoadApplication { source })?
        .ok_or_else(|| DeployReleaseError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        })
}

struct FailedExecution {
    code: &'static str,
    source: Box<dyn Error>,
    container_id: Option<String>,
    runtime_id: Option<String>,
    failure_persisted: bool,
    unit_name: Option<String>,
    port_reserved: bool,
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
    advance_deployment(connection, deployment_id, DeploymentTransition::Start).map_err(
        |source| failure_needing_persistence("deployment_transition_failed", source, None, None),
    )?;
    progress.state_changed(deployment_id, DeploymentStatus::Starting);
    progress.started(
        DeploymentStep::CreateContainer,
        format!("image {image_reference}"),
    );
    let host_port = reserve_port(connection, &specification.application_id, deployment_id)
        .map_err(|source| {
            failure_needing_persistence("runtime_port_allocation_failed", source, None, None)
        })?;
    let unit = write_unit(
        &specification.application_name,
        deployment_id,
        image_reference,
        specification.container_port,
        host_port,
        source_revision,
    )
    .map_err(|source| {
        candidate_failure(
            "runtime_unit_creation_failed",
            source,
            None,
            None,
            None,
            true,
        )
    })?;
    daemon_reload().map_err(|source| {
        candidate_failure(
            "runtime_unit_reload_failed",
            source,
            None,
            None,
            Some(&unit),
            true,
        )
    })?;
    progress.completed(
        DeploymentStep::CreateContainer,
        format!("unit {unit}, endpoint 127.0.0.1:{host_port}"),
    );

    progress.started(DeploymentStep::StartContainer, format!("unit {unit}"));
    start(&unit).map_err(|source| {
        candidate_failure(
            "runtime_start_failed",
            source,
            None,
            None,
            Some(&unit),
            true,
        )
    })?;
    let name = container_name(&specification.application_name, deployment_id);
    let container_id = resolve_container_id(&name).map_err(|source| {
        candidate_failure(
            "runtime_resolution_failed",
            source,
            None,
            None,
            Some(&unit),
            true,
        )
    })?;
    progress.completed(
        DeploymentStep::StartContainer,
        format!("container {container_id}"),
    );

    let (runtime_id, finished_at) = execute_candidate(
        connection,
        deployment_id,
        specification,
        &container_id,
        source_revision,
        public_configuration,
        progress,
    )
    .map_err(|mut failed| {
        failed.unit_name = Some(unit);
        failed.port_reserved = true;
        failed
    })?;

    Ok((runtime_id, name, finished_at))
}

fn execute_candidate(
    connection: &mut Connection,
    deployment_id: &str,
    specification: &DeploymentSpecification,
    container_id: &str,
    commit_sha: &str,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<(String, String), FailedExecution> {
    progress.started(
        DeploymentStep::ObserveContainer,
        format!("container {container_id}"),
    );
    let observation =
        observe_container(container_id, specification.container_port).map_err(|source| {
            failure_needing_persistence(
                "runtime_observation_failed",
                source,
                Some(container_id),
                None,
            )
        })?;
    if observation.state != ObservedRuntimeState::Running {
        return Err(failure_needing_persistence(
            "runtime_observation_failed",
            RuntimeObservationFailure::NotRunning {
                actual: observation.state,
            },
            Some(container_id),
            None,
        ));
    }
    let endpoint = observation.endpoint.ok_or_else(|| {
        failure_needing_persistence(
            "runtime_observation_failed",
            RuntimeObservationFailure::MissingEndpoint,
            Some(container_id),
            None,
        )
    })?;
    progress.completed(
        DeploymentStep::ObserveContainer,
        format!("state Running, endpoint {endpoint}"),
    );
    progress.started(
        DeploymentStep::RegisterCandidate,
        format!("container {container_id}, endpoint {endpoint}"),
    );
    let runtime = register_candidate_runtime(
        connection,
        deployment_id,
        container_id,
        endpoint,
        specification.container_port,
    )
    .map_err(|source| {
        failure_needing_persistence(
            "runtime_registration_failed",
            source,
            Some(container_id),
            None,
        )
    })?;
    progress.completed(
        DeploymentStep::RegisterCandidate,
        format!("runtime {}", runtime.id),
    );
    consume_port_reservation(connection, deployment_id).map_err(|source| {
        candidate_failure(
            "runtime_port_persistence_failed",
            source,
            Some(container_id),
            Some(&runtime.id),
            Some(&unit_name(&specification.application_name, deployment_id)),
            false,
        )
    })?;
    advance_deployment(
        connection,
        deployment_id,
        DeploymentTransition::RuntimeRunning,
    )
    .map_err(|source| {
        failure_needing_persistence(
            "deployment_transition_failed",
            source,
            Some(container_id),
            Some(&runtime.id),
        )
    })?;
    progress.state_changed(deployment_id, DeploymentStatus::Verifying);
    let previous_runtime =
        load_previous_runtime(connection, &specification.application_id, &runtime.id).map_err(
            |source| {
                candidate_failure(
                    "runtime_reconciliation_failed",
                    source,
                    Some(container_id),
                    Some(&runtime.id),
                    Some(&unit_name(&specification.application_name, deployment_id)),
                    false,
                )
            },
        )?;
    if specification.visibility == Visibility::Public {
        let Some(public_configuration) = public_configuration else {
            return Err(failure_needing_persistence(
                "public_configuration_missing",
                DeployReleaseError::PublicApplication {
                    application_id: specification.application_id.clone(),
                },
                Some(container_id),
                Some(&runtime.id),
            ));
        };
        let completed = execute_public_candidate(
            connection,
            specification,
            &runtime,
            commit_sha,
            public_configuration,
            progress,
        );
        if completed.is_ok() {
            finalize_runtime_supervision(
                connection,
                specification,
                deployment_id,
                previous_runtime.as_ref(),
            );
        }
        return completed;
    }

    progress.started(
        DeploymentStep::HealthCheckAndPromotion,
        format!(
            "runtime {}, path {}, expected status {}",
            runtime.id, specification.health_path, specification.expected_status
        ),
    );
    let promoted = promote_internal_candidate(
        connection,
        &runtime.id,
        &specification.health_path,
        specification.expected_status,
    )
    .map_err(|source| {
        if matches!(
            &source,
            PromoteInternalCandidateError::CandidateUnhealthy { .. }
        ) {
            failure_already_persisted("health_check_failed", source, container_id, &runtime.id)
        } else {
            failure_needing_persistence(
                "candidate_promotion_failed",
                source,
                Some(container_id),
                Some(&runtime.id),
            )
        }
    })?;
    progress.completed(
        DeploymentStep::HealthCheckAndPromotion,
        format!("runtime {} promoted to Current", runtime.id),
    );
    progress.state_changed(deployment_id, DeploymentStatus::Succeeded);
    finalize_runtime_supervision(
        connection,
        specification,
        deployment_id,
        previous_runtime.as_ref(),
    );

    Ok((runtime.id, promoted.finished_at))
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
        container_id: Some(container_id.to_owned()),
        runtime_id: Some(runtime_id.to_owned()),
        failure_persisted: false,
        unit_name: None,
        port_reserved: false,
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
    let cleanup_error =
        if failed.container_id.is_some() || failed.unit_name.is_some() || failed.port_reserved {
            progress.started(
                DeploymentStep::CleanupCandidate,
                format!("deployment {deployment_id}"),
            );
            match cleanup_candidate(
                connection,
                deployment_id,
                failed.unit_name.as_deref(),
                failed.container_id.as_deref(),
                failed.runtime_id.as_deref(),
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
    FailedExecution {
        code,
        source: Box::new(source),
        container_id: container_id.map(str::to_owned),
        runtime_id: runtime_id.map(str::to_owned),
        failure_persisted: false,
        unit_name: None,
        port_reserved: false,
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
    FailedExecution {
        code,
        source: Box::new(source),
        container_id: container_id.map(str::to_owned),
        runtime_id: runtime_id.map(str::to_owned),
        failure_persisted: false,
        unit_name: unit_name.map(str::to_owned),
        port_reserved,
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
        container_id: Some(container_id.to_owned()),
        runtime_id: Some(runtime_id.to_owned()),
        failure_persisted: true,
        unit_name: None,
        port_reserved: false,
    }
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

struct PreviousRuntime {
    runtime_id: String,
    deployment_id: String,
    external_runtime_id: String,
}

fn load_previous_runtime(
    connection: &Connection,
    application_id: &str,
    candidate_runtime_id: &str,
) -> Result<Option<PreviousRuntime>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT id, deployment_id, external_runtime_id
             FROM runtime_instances
             WHERE application_id = ?1
               AND state = 'running'
               AND removed_at IS NULL
               AND id != ?2",
            [application_id, candidate_runtime_id],
            |row| {
                Ok(PreviousRuntime {
                    runtime_id: row.get(0)?,
                    deployment_id: row.get(1)?,
                    external_runtime_id: row.get(2)?,
                })
            },
        )
        .optional()
}

fn finalize_runtime_supervision(
    connection: &Connection,
    specification: &DeploymentSpecification,
    deployment_id: &str,
    previous: Option<&PreviousRuntime>,
) {
    let current_unit = unit_name(&specification.application_name, deployment_id);
    if let Err(source) = enable(&current_unit) {
        eprintln!(
            "warning: promoted runtime is active but its Quadlet unit could not be enabled: {source}"
        );
    }
    let Some(previous) = previous else {
        return;
    };
    let previous_unit = unit_name(&specification.application_name, &previous.deployment_id);
    let retirement = (|| -> Result<(), QuadletError> {
        stop(&previous_unit)?;
        disable(&previous_unit)?;
        remove_unit(&previous_unit)?;
        daemon_reload()?;
        Ok(())
    })();
    if let Err(source) = retirement {
        eprintln!(
            "warning: previous runtime {} could not be retired: {source}",
            previous.runtime_id
        );
        return;
    }
    if let Err(source) = remove_container(&previous.external_runtime_id) {
        eprintln!(
            "warning: previous runtime {} unit was retired but its container could not be removed: {source}",
            previous.runtime_id
        );
        return;
    }
    if let Err(source) = connection.execute(
        "UPDATE runtime_instances
         SET state = 'removed', removed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1 AND state = 'stopped' AND removed_at IS NULL",
        [&previous.runtime_id],
    ) {
        eprintln!(
            "warning: previous runtime {} was retired but could not be marked removed: {source}",
            previous.runtime_id
        );
    }
}

fn cleanup_candidate(
    connection: &Connection,
    deployment_id: &str,
    unit: Option<&str>,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
) -> Result<(), CandidateCleanupError> {
    if let Some(runtime_id) = runtime_id {
        let state = connection
            .query_row(
                "SELECT state FROM runtime_instances WHERE id = ?1",
                [runtime_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|source| CandidateCleanupError::Persistence { source })?;
        // A promotion error may have an uncertain external outcome. Never remove an
        // already active runtime.
        if state.as_deref().is_some_and(|state| state != "starting") {
            return Ok(());
        }
    }

    if let Some(unit) = unit {
        stop(unit).map_err(|source| CandidateCleanupError::StopUnit { source })?;
        remove_unit(unit).map_err(|source| CandidateCleanupError::RemoveUnit { source })?;
        daemon_reload().map_err(|source| CandidateCleanupError::ReloadUnits { source })?;
    }
    if let Some(container_id) = container_id {
        remove_container(container_id)
            .map_err(|source| CandidateCleanupError::RemoveContainer { source })?;
    }
    if let Some(runtime_id) = runtime_id {
        connection
            .execute(
                "UPDATE runtime_instances
                 SET last_observed_state = 'missing',
                     last_observed_at = CURRENT_TIMESTAMP,
                     removed_at = CURRENT_TIMESTAMP,
                     updated_at = CURRENT_TIMESTAMP
                  WHERE id = ?1 AND state = 'starting' AND removed_at IS NULL",
                [runtime_id],
            )
            .map_err(|source| CandidateCleanupError::Persistence { source })?;
    }
    release_port(connection, deployment_id)
        .map_err(|source| CandidateCleanupError::ReleasePort { source })?;
    Ok(())
}
