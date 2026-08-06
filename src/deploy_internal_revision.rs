use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::create_deployment::{CreateDeploymentError, DeploymentStatus, create_deployment};
use crate::git_source::{ResolveCommitError, create_checkout, resolve_commit};
use crate::health_check::{HealthCheckError, HealthCheckResult, check_internal_health};
use crate::local_build::build_image;
use crate::local_runtime::{
    ContainerObservation, ControlContainerError, ObserveContainerError, ObservedRuntimeState,
    container_name, create_container, observe_container, remove_container, start_container,
};
use crate::promote_internal_candidate::{
    PromoteInternalCandidateError, promote_internal_candidate,
};
use crate::register_candidate_runtime::register_candidate_runtime;
use crate::transition_deployment::{
    DeploymentTransition, TransitionDeploymentError, advance_deployment, fail_deployment,
};

#[derive(Debug, PartialEq, Eq)]
pub struct DeployedInternalRevision {
    pub deployment_id: String,
    pub runtime_id: String,
    pub container_name: String,
    pub commit_sha: String,
    pub finished_at: String,
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
pub enum DeployInternalRevisionError {
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
    RemoveContainer { source: ControlContainerError },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for DeployInternalRevisionError {
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

impl Error for DeployInternalRevisionError {
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
            | Self::ExistingRuntimeUnhealthy { .. } => None,
        }
    }
}

impl fmt::Display for CandidateCleanupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RemoveContainer { source } => write!(formatter, "{source}"),
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
            Self::Persistence { source } => Some(source),
        }
    }
}

pub fn deploy_internal_revision(
    connection: &mut Connection,
    application_id: &str,
    repository_path: &Path,
    revision: &str,
    workspace_root: &Path,
) -> Result<DeployedInternalRevision, DeployInternalRevisionError> {
    let mut progress = ProgressReporter::disabled();
    deploy_internal_revision_reporting(
        connection,
        application_id,
        repository_path,
        revision,
        workspace_root,
        &mut progress,
    )
}

pub fn deploy_internal_revision_with_progress(
    connection: &mut Connection,
    application_id: &str,
    repository_path: &Path,
    revision: &str,
    workspace_root: &Path,
    progress: &mut dyn FnMut(DeploymentProgress),
) -> Result<DeployedInternalRevision, DeployInternalRevisionError> {
    let mut progress = ProgressReporter::enabled(progress);
    deploy_internal_revision_reporting(
        connection,
        application_id,
        repository_path,
        revision,
        workspace_root,
        &mut progress,
    )
}

fn deploy_internal_revision_reporting(
    connection: &mut Connection,
    application_id: &str,
    repository_path: &Path,
    revision: &str,
    workspace_root: &Path,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeployedInternalRevision, DeployInternalRevisionError> {
    progress.started(
        DeploymentStep::LoadSpecification,
        format!("application {application_id}"),
    );
    let specification = load_specification(connection, application_id)?;
    progress.completed(
        DeploymentStep::LoadSpecification,
        format!(
            "application {}, visibility {}",
            specification.application_name, specification.visibility
        ),
    );
    if specification.visibility != "internal" {
        return Err(DeployInternalRevisionError::PublicApplication {
            application_id: application_id.to_owned(),
        });
    }

    progress.started(
        DeploymentStep::ResolveRevision,
        format!("{revision} in {}", repository_path.display()),
    );
    let commit_sha = resolve_commit(repository_path, revision)
        .map_err(|source| DeployInternalRevisionError::ResolveRevision { source })?;
    progress.completed(
        DeploymentStep::ResolveRevision,
        format!("commit {commit_sha}"),
    );
    if let Some(deployed) = reconcile_existing_runtime(
        connection,
        application_id,
        &specification,
        &commit_sha,
        progress,
    )? {
        return Ok(deployed);
    }
    progress.started(
        DeploymentStep::CreateDeployment,
        format!("commit {commit_sha}"),
    );
    let (_, deployment) =
        create_deployment(connection, application_id, &commit_sha, Some(revision))
            .map_err(|source| DeployInternalRevisionError::CreateDeployment { source })?;
    progress.completed(
        DeploymentStep::CreateDeployment,
        format!("deployment {}", deployment.id),
    );
    progress.state_changed(&deployment.id, DeploymentStatus::Pending);

    let execution = execute_deployment(
        connection,
        &deployment.id,
        &specification,
        repository_path,
        &commit_sha,
        workspace_root,
        progress,
    );
    match execution {
        Ok((runtime_id, container_name, finished_at)) => Ok(DeployedInternalRevision {
            deployment_id: deployment.id,
            runtime_id,
            container_name,
            commit_sha,
            finished_at,
        }),
        Err(failed) => finish_failed_deployment(connection, &deployment.id, failed, progress),
    }
}

struct DeploymentSpecification {
    application_name: String,
    containerfile: PathBuf,
    context: PathBuf,
    container_port: u16,
    health_path: String,
    expected_status: u16,
    visibility: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExistingRuntimeRole {
    Current,
    Previous,
}

impl ExistingRuntimeRole {
    fn database_value(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Previous => "previous",
        }
    }
}

struct ExistingRuntime {
    runtime_id: String,
    deployment_id: String,
    external_runtime_id: String,
    container_port: u16,
    finished_at: String,
    role: ExistingRuntimeRole,
}

fn reconcile_existing_runtime(
    connection: &mut Connection,
    application_id: &str,
    specification: &DeploymentSpecification,
    commit_sha: &str,
    progress: &mut ProgressReporter<'_>,
) -> Result<Option<DeployedInternalRevision>, DeployInternalRevisionError> {
    let Some(runtime) = load_existing_runtime(connection, application_id, commit_sha)? else {
        return Ok(None);
    };
    progress.started(
        DeploymentStep::ReconcileRuntime,
        format!(
            "runtime {}, role {:?}, commit {commit_sha}",
            runtime.runtime_id, runtime.role
        ),
    );

    let observation = observe_container(&runtime.external_runtime_id, runtime.container_port)
        .map_err(
            |source| DeployInternalRevisionError::ObserveExistingRuntime {
                runtime_id: runtime.runtime_id.clone(),
                source,
            },
        )?;
    persist_existing_observation(connection, &runtime, &observation)?;

    let endpoint = match observation.state {
        ObservedRuntimeState::Missing => {
            progress.completed(
                DeploymentStep::ReconcileRuntime,
                format!(
                    "runtime {} is missing; a new deployment is required",
                    runtime.runtime_id
                ),
            );
            return Ok(None);
        }
        ObservedRuntimeState::Running => observation.endpoint.ok_or_else(|| {
            DeployInternalRevisionError::ExistingRuntimeUnavailable {
                runtime_id: runtime.runtime_id.clone(),
                state: ObservedRuntimeState::Running,
            }
        })?,
        ObservedRuntimeState::Created | ObservedRuntimeState::Stopped => {
            start_container(&runtime.external_runtime_id).map_err(|source| {
                DeployInternalRevisionError::StartExistingRuntime {
                    runtime_id: runtime.runtime_id.clone(),
                    source,
                }
            })?;
            let observation =
                observe_container(&runtime.external_runtime_id, runtime.container_port).map_err(
                    |source| DeployInternalRevisionError::ObserveExistingRuntime {
                        runtime_id: runtime.runtime_id.clone(),
                        source,
                    },
                )?;
            persist_existing_observation(connection, &runtime, &observation)?;
            match observation {
                ContainerObservation {
                    state: ObservedRuntimeState::Running,
                    endpoint: Some(endpoint),
                } => endpoint,
                observation => {
                    return Err(DeployInternalRevisionError::ExistingRuntimeUnavailable {
                        runtime_id: runtime.runtime_id,
                        state: observation.state,
                    });
                }
            }
        }
        state => {
            return Err(DeployInternalRevisionError::ExistingRuntimeUnavailable {
                runtime_id: runtime.runtime_id,
                state,
            });
        }
    };

    let health = check_internal_health(
        endpoint,
        &specification.health_path,
        specification.expected_status,
    )
    .map_err(|source| DeployInternalRevisionError::CheckExistingRuntime {
        runtime_id: runtime.runtime_id.clone(),
        source,
    })?;
    match health {
        HealthCheckResult::Healthy { .. } => {}
        result @ HealthCheckResult::Unhealthy { .. } => {
            return Err(DeployInternalRevisionError::ExistingRuntimeUnhealthy {
                runtime_id: runtime.runtime_id,
                result,
            });
        }
    }

    if runtime.role == ExistingRuntimeRole::Previous {
        reactivate_previous_runtime(connection, application_id, &runtime.runtime_id)?;
    }

    progress.completed(
        DeploymentStep::ReconcileRuntime,
        match runtime.role {
            ExistingRuntimeRole::Current => {
                format!("runtime {} is running and healthy", runtime.runtime_id)
            }
            ExistingRuntimeRole::Previous => {
                format!("runtime {} reactivated as Current", runtime.runtime_id)
            }
        },
    );
    Ok(Some(DeployedInternalRevision {
        deployment_id: runtime.deployment_id,
        runtime_id: runtime.runtime_id,
        container_name: container_name(&specification.application_name, commit_sha),
        commit_sha: commit_sha.to_owned(),
        finished_at: runtime.finished_at,
    }))
}

fn load_existing_runtime(
    connection: &Connection,
    application_id: &str,
    commit_sha: &str,
) -> Result<Option<ExistingRuntime>, DeployInternalRevisionError> {
    let runtime = connection
        .query_row(
            "SELECT
                runtime_instances.id,
                runtime_instances.deployment_id,
                runtime_instances.external_runtime_id,
                runtime_instances.container_port,
                deployments.finished_at,
                runtime_instances.role = 'current'
             FROM runtime_instances
             JOIN revisions ON revisions.id = runtime_instances.revision_id
             JOIN deployments ON deployments.id = runtime_instances.deployment_id
             WHERE runtime_instances.application_id = ?1
               AND revisions.commit_sha = ?2
               AND runtime_instances.role IN ('current', 'previous')
               AND runtime_instances.removed_at IS NULL
               AND deployments.status = 'succeeded'
               AND deployments.finished_at IS NOT NULL
             ORDER BY
                CASE runtime_instances.role WHEN 'current' THEN 0 ELSE 1 END,
                runtime_instances.created_at DESC
             LIMIT 1",
            params![application_id, commit_sha],
            |row| {
                let role = if row.get::<_, bool>(5)? {
                    ExistingRuntimeRole::Current
                } else {
                    ExistingRuntimeRole::Previous
                };
                Ok(ExistingRuntime {
                    runtime_id: row.get(0)?,
                    deployment_id: row.get(1)?,
                    external_runtime_id: row.get(2)?,
                    container_port: row.get(3)?,
                    finished_at: row.get(4)?,
                    role,
                })
            },
        )
        .optional()
        .map_err(|source| DeployInternalRevisionError::LoadExistingRuntime { source })?;
    Ok(runtime)
}

fn persist_existing_observation(
    connection: &Connection,
    runtime: &ExistingRuntime,
    observation: &ContainerObservation,
) -> Result<(), DeployInternalRevisionError> {
    let state = observed_state_database_value(&observation.state);
    let role = runtime.role.database_value();
    let updated = if observation.state == ObservedRuntimeState::Missing {
        connection.execute(
            "UPDATE runtime_instances
             SET last_observed_state = 'missing',
                 last_observed_at = CURRENT_TIMESTAMP,
                 removed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND role = ?2 AND removed_at IS NULL",
            params![runtime.runtime_id, role],
        )
    } else if let Some(endpoint) = observation.endpoint {
        connection.execute(
            "UPDATE runtime_instances
             SET last_observed_state = ?2,
                 host_port = ?3,
                 last_observed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND role = ?4 AND removed_at IS NULL",
            params![runtime.runtime_id, state, endpoint.port(), role],
        )
    } else {
        connection.execute(
            "UPDATE runtime_instances
             SET last_observed_state = ?2,
                 last_observed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND role = ?3 AND removed_at IS NULL",
            params![runtime.runtime_id, state, role],
        )
    }
    .map_err(
        |source| DeployInternalRevisionError::PersistExistingRuntime {
            runtime_id: runtime.runtime_id.clone(),
            source,
        },
    )?;
    if updated != 1 {
        return Err(DeployInternalRevisionError::ExistingRuntimeChanged {
            runtime_id: runtime.runtime_id.clone(),
        });
    }
    Ok(())
}

fn reactivate_previous_runtime(
    connection: &mut Connection,
    application_id: &str,
    runtime_id: &str,
) -> Result<(), DeployInternalRevisionError> {
    // Health validation happens before this transaction. The immediate transaction then
    // makes the role swap indivisible, so the unique Current role is never ambiguous.
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(
            |source| DeployInternalRevisionError::ReactivatePreviousRuntime {
                runtime_id: runtime_id.to_owned(),
                source,
            },
        )?;
    let current_runtime_id = transaction
        .query_row(
            "SELECT id FROM runtime_instances
             WHERE application_id = ?1
               AND role = 'current'
               AND removed_at IS NULL",
            [application_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(
            |source| DeployInternalRevisionError::ReactivatePreviousRuntime {
                runtime_id: runtime_id.to_owned(),
                source,
            },
        )?;
    if let Some(current_runtime_id) = current_runtime_id {
        transaction
            .execute(
                "UPDATE runtime_instances
                 SET role = 'previous', updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND role = 'current' AND removed_at IS NULL",
                [current_runtime_id],
            )
            .map_err(
                |source| DeployInternalRevisionError::ReactivatePreviousRuntime {
                    runtime_id: runtime_id.to_owned(),
                    source,
                },
            )?;
    }
    let updated = transaction
        .execute(
            "UPDATE runtime_instances
             SET role = 'current', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND role = 'previous' AND removed_at IS NULL",
            [runtime_id],
        )
        .map_err(
            |source| DeployInternalRevisionError::ReactivatePreviousRuntime {
                runtime_id: runtime_id.to_owned(),
                source,
            },
        )?;
    if updated != 1 {
        return Err(DeployInternalRevisionError::ExistingRuntimeChanged {
            runtime_id: runtime_id.to_owned(),
        });
    }
    transaction.commit().map_err(|source| {
        DeployInternalRevisionError::ReactivatePreviousRuntime {
            runtime_id: runtime_id.to_owned(),
            source,
        }
    })?;
    Ok(())
}

fn observed_state_database_value(state: &ObservedRuntimeState) -> &'static str {
    match state {
        ObservedRuntimeState::Missing => "missing",
        ObservedRuntimeState::Created => "created",
        ObservedRuntimeState::Starting => "starting",
        ObservedRuntimeState::Running => "running",
        ObservedRuntimeState::Stopping => "stopping",
        ObservedRuntimeState::Stopped => "stopped",
        ObservedRuntimeState::Failed => "failed",
        ObservedRuntimeState::Unknown { .. } => "unknown",
    }
}

fn load_specification(
    connection: &Connection,
    application_id: &str,
) -> Result<DeploymentSpecification, DeployInternalRevisionError> {
    connection
        .query_row(
            "SELECT
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
                Ok(DeploymentSpecification {
                    application_name: row.get(0)?,
                    containerfile: PathBuf::from(row.get::<_, String>(1)?),
                    context: PathBuf::from(row.get::<_, String>(2)?),
                    container_port: row.get(3)?,
                    health_path: row.get(4)?,
                    expected_status: row.get(5)?,
                    visibility: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|source| DeployInternalRevisionError::LoadApplication { source })?
        .ok_or_else(|| DeployInternalRevisionError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        })
}

struct FailedExecution {
    code: &'static str,
    source: Box<dyn Error>,
    container_id: Option<String>,
    runtime_id: Option<String>,
    failure_persisted: bool,
}

fn execute_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    specification: &DeploymentSpecification,
    repository_path: &Path,
    commit_sha: &str,
    workspace_root: &Path,
    progress: &mut ProgressReporter<'_>,
) -> Result<(String, String, String), FailedExecution> {
    advance_deployment(connection, deployment_id, DeploymentTransition::Start).map_err(
        |source| failure_needing_persistence("deployment_transition_failed", source, None, None),
    )?;
    progress.state_changed(deployment_id, DeploymentStatus::PreparingSource);
    let checkout_path = workspace_root.join(deployment_id);
    progress.started(
        DeploymentStep::PrepareCheckout,
        checkout_path.display().to_string(),
    );
    fs::create_dir_all(workspace_root).map_err(|source| {
        failure_needing_persistence("source_preparation_failed", source, None, None)
    })?;
    create_checkout(repository_path, commit_sha, &checkout_path).map_err(|source| {
        failure_needing_persistence("source_preparation_failed", source, None, None)
    })?;
    progress.completed(
        DeploymentStep::PrepareCheckout,
        checkout_path.display().to_string(),
    );
    advance_deployment(
        connection,
        deployment_id,
        DeploymentTransition::SourcePrepared,
    )
    .map_err(|source| {
        failure_needing_persistence("deployment_transition_failed", source, None, None)
    })?;
    progress.state_changed(deployment_id, DeploymentStatus::Building);
    progress.started(
        DeploymentStep::BuildImage,
        format!(
            "application {}, commit {commit_sha}",
            specification.application_name
        ),
    );
    let image = build_image(
        &checkout_path,
        &specification.application_name,
        commit_sha,
        &specification.containerfile,
        &specification.context,
    )
    .map_err(|source| failure_needing_persistence("image_build_failed", source, None, None))?;
    progress.completed(
        DeploymentStep::BuildImage,
        format!("image {}", image.reference),
    );
    advance_deployment(connection, deployment_id, DeploymentTransition::ImageBuilt).map_err(
        |source| failure_needing_persistence("deployment_transition_failed", source, None, None),
    )?;
    progress.state_changed(deployment_id, DeploymentStatus::Starting);
    progress.started(
        DeploymentStep::CreateContainer,
        format!("image {}", image.reference),
    );
    let container = create_container(
        &image.reference,
        &specification.application_name,
        commit_sha,
        specification.container_port,
    )
    .map_err(|source| failure_needing_persistence("runtime_creation_failed", source, None, None))?;
    progress.completed(
        DeploymentStep::CreateContainer,
        format!("container {}", container.id),
    );

    let (runtime_id, finished_at) = execute_candidate(
        connection,
        deployment_id,
        specification,
        &container.id,
        progress,
    )?;

    Ok((runtime_id, container.name, finished_at))
}

fn execute_candidate(
    connection: &mut Connection,
    deployment_id: &str,
    specification: &DeploymentSpecification,
    container_id: &str,
    progress: &mut ProgressReporter<'_>,
) -> Result<(String, String), FailedExecution> {
    progress.started(
        DeploymentStep::StartContainer,
        format!("container {container_id}"),
    );
    start_container(container_id).map_err(|source| {
        failure_needing_persistence("runtime_start_failed", source, Some(container_id), None)
    })?;
    progress.completed(
        DeploymentStep::StartContainer,
        format!("container {container_id}"),
    );
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
    progress.state_changed(deployment_id, DeploymentStatus::VerifyingInternal);
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

    Ok((runtime.id, promoted.finished_at))
}

fn finish_failed_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    failed: FailedExecution,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeployedInternalRevision, DeployInternalRevisionError> {
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
    let cleanup_error = if let Some(container_id) = failed.container_id.as_deref() {
        progress.started(
            DeploymentStep::CleanupCandidate,
            format!("container {container_id}"),
        );
        match cleanup_candidate(connection, container_id, failed.runtime_id.as_deref()) {
            Ok(()) => {
                progress.completed(
                    DeploymentStep::CleanupCandidate,
                    format!("container {container_id}"),
                );
                None
            }
            Err(source) => Some(source),
        }
    } else {
        None
    };

    if let Some(source) = cleanup_error {
        return Err(DeployInternalRevisionError::Cleanup {
            deployment_id: deployment_id.to_owned(),
            failure,
            source: Box::new(source),
        });
    }
    if let Some(source) = record_error {
        return Err(DeployInternalRevisionError::RecordFailure {
            deployment_id: deployment_id.to_owned(),
            failure,
            source,
        });
    }

    Err(DeployInternalRevisionError::DeploymentFailed {
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

fn cleanup_candidate(
    connection: &Connection,
    container_id: &str,
    runtime_id: Option<&str>,
) -> Result<(), CandidateCleanupError> {
    if let Some(runtime_id) = runtime_id {
        let role = connection
            .query_row(
                "SELECT role FROM runtime_instances WHERE id = ?1",
                [runtime_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|source| CandidateCleanupError::Persistence { source })?;
        // A promotion error may have an uncertain external outcome. Never remove a runtime
        // that the database already recognizes as Current or Previous during reconciliation.
        if role.as_deref().is_some_and(|role| role != "candidate") {
            return Ok(());
        }
    }

    remove_container(container_id)
        .map_err(|source| CandidateCleanupError::RemoveContainer { source })?;
    if let Some(runtime_id) = runtime_id {
        connection
            .execute(
                "UPDATE runtime_instances
                 SET last_observed_state = 'missing',
                     last_observed_at = CURRENT_TIMESTAMP,
                     removed_at = CURRENT_TIMESTAMP,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND role = 'candidate' AND removed_at IS NULL",
                [runtime_id],
            )
            .map_err(|source| CandidateCleanupError::Persistence { source })?;
    }
    Ok(())
}
