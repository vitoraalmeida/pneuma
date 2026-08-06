use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::create_deployment::{CreateDeploymentError, create_deployment};
use crate::git_source::{ResolveCommitError, create_checkout, resolve_commit};
use crate::local_build::build_image;
use crate::local_runtime::{
    ControlContainerError, ObservedRuntimeState, create_container, observe_container,
    remove_container, start_container,
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
    pub commit_sha: String,
    pub finished_at: String,
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
            Self::CreateDeployment { source } => Some(source),
            Self::DeploymentFailed { source, .. } => Some(source.as_ref()),
            Self::RecordFailure { source, .. } => Some(source),
            Self::Cleanup { source, .. } => Some(source.as_ref()),
            Self::ApplicationNotFound { .. } | Self::PublicApplication { .. } => None,
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
    let specification = load_specification(connection, application_id)?;
    if specification.visibility != "internal" {
        return Err(DeployInternalRevisionError::PublicApplication {
            application_id: application_id.to_owned(),
        });
    }

    let commit_sha = resolve_commit(repository_path, revision)
        .map_err(|source| DeployInternalRevisionError::ResolveRevision { source })?;
    let (_, deployment) =
        create_deployment(connection, application_id, &commit_sha, Some(revision))
            .map_err(|source| DeployInternalRevisionError::CreateDeployment { source })?;

    let execution = execute_deployment(
        connection,
        &deployment.id,
        &specification,
        repository_path,
        &commit_sha,
        workspace_root,
    );
    match execution {
        Ok((runtime_id, finished_at)) => Ok(DeployedInternalRevision {
            deployment_id: deployment.id,
            runtime_id,
            commit_sha,
            finished_at,
        }),
        Err(failed) => finish_failed_deployment(connection, &deployment.id, failed),
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
) -> Result<(String, String), FailedExecution> {
    advance_deployment(connection, deployment_id, DeploymentTransition::Start).map_err(
        |source| failure_needing_persistence("deployment_transition_failed", source, None, None),
    )?;
    fs::create_dir_all(workspace_root).map_err(|source| {
        failure_needing_persistence("source_preparation_failed", source, None, None)
    })?;
    let checkout_path = workspace_root.join(deployment_id);
    create_checkout(repository_path, commit_sha, &checkout_path).map_err(|source| {
        failure_needing_persistence("source_preparation_failed", source, None, None)
    })?;
    advance_deployment(
        connection,
        deployment_id,
        DeploymentTransition::SourcePrepared,
    )
    .map_err(|source| {
        failure_needing_persistence("deployment_transition_failed", source, None, None)
    })?;
    let image = build_image(
        &checkout_path,
        &specification.application_name,
        commit_sha,
        &specification.containerfile,
        &specification.context,
    )
    .map_err(|source| failure_needing_persistence("image_build_failed", source, None, None))?;
    advance_deployment(connection, deployment_id, DeploymentTransition::ImageBuilt).map_err(
        |source| failure_needing_persistence("deployment_transition_failed", source, None, None),
    )?;
    let container = create_container(
        &image.reference,
        &specification.application_name,
        commit_sha,
        specification.container_port,
    )
    .map_err(|source| failure_needing_persistence("runtime_creation_failed", source, None, None))?;

    execute_candidate(connection, deployment_id, specification, &container.id)
}

fn execute_candidate(
    connection: &mut Connection,
    deployment_id: &str,
    specification: &DeploymentSpecification,
    container_id: &str,
) -> Result<(String, String), FailedExecution> {
    start_container(container_id).map_err(|source| {
        failure_needing_persistence("runtime_start_failed", source, Some(container_id), None)
    })?;
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

    Ok((runtime.id, promoted.finished_at))
}

fn finish_failed_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    failed: FailedExecution,
) -> Result<DeployedInternalRevision, DeployInternalRevisionError> {
    let failure = failed.source.to_string();
    let record_error = if failed.failure_persisted {
        None
    } else {
        fail_deployment(connection, deployment_id, failed.code, &failure).err()
    };
    let cleanup_error = failed.container_id.as_deref().and_then(|container_id| {
        cleanup_candidate(connection, container_id, failed.runtime_id.as_deref()).err()
    });

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
