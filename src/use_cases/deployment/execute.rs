use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use rusqlite::Connection;

use super::activation::{PublicActivationError, PublicActivationInput, activate_public_candidate};
use super::candidate::{
    CandidateStartError, CandidateStartInput, StartedCandidate, start_candidate,
};
use super::cleanup::{
    CandidateCleanupError, CandidateResources, cleanup_failed_candidate, load_previous_runtime,
    retire_previous_runtime,
};
use super::create::{CreateDeploymentError, create_deployment_with_source_revision_and_ownership};
use super::progress::{DeploymentProgress, DeploymentStep, ProgressReporter};
use super::promotion::{PromoteInternalCandidateError, promote_internal_candidate};
use super::transition::{TransitionDeploymentError, fail_deployment};
use crate::adapters::application_lock::{ApplicationLock, ApplicationLockError};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::operation_store;
use crate::adapters::test_gate::wait_for_test_gate;
use crate::domain::application::ApplicationDeploymentSpecification;
use crate::domain::deployment::{DeploymentStatus, DeploymentType, SourceRevision};
use crate::domain::exposure::Visibility;
use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId};
use crate::domain::release::{OciArtifact, Release};

#[derive(Debug, PartialEq, Eq)]
// Describes the successfully promoted runtime returned to deployment callers.
pub struct DeploymentResult {
    pub deployment_id: DeploymentId,
    pub runtime_id: RuntimeInstanceId,
    pub container_name: String,
    pub artifact: OciArtifact,
    pub source_revision: Option<SourceRevision>,
    pub finished_at: String,
}

#[derive(Debug, PartialEq, Eq)]
// Supplies host-managed Caddy paths required only for public application activation.
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
        source: ApplicationStoreError,
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
    OperationLock {
        source: ApplicationLockError,
    },
    OperationToken {
        source: operation_store::OperationStoreError,
    },
    OperationInProgress {
        application_id: String,
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
            Self::OperationLock { source } => {
                write!(formatter, "failed to serialize deployment: {source}")
            }
            Self::OperationToken { source } => {
                write!(formatter, "failed to create deployment ownership: {source}")
            }
            Self::OperationInProgress { application_id } => write!(
                formatter,
                "application `{application_id}` already has an operation in progress"
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
            Self::OperationLock { source } => Some(source),
            Self::OperationToken { source } => Some(source),
            Self::ApplicationNotFound { .. }
            | Self::PublicApplication { .. }
            | Self::OperationInProgress { .. } => None,
        }
    }
}

// Deploys a release without progress callbacks while preserving the full execution workflow.
pub(crate) fn deploy_release(
    connection: &mut Connection,
    application_id: &ApplicationId,
    release: &Release,
    deployment_type: DeploymentType,
    source_revision: Option<&SourceRevision>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, DeployReleaseError> {
    let mut progress = ProgressReporter::disabled();
    deploy_release_reporting(
        connection,
        application_id,
        release,
        deployment_type,
        source_revision,
        public_configuration,
        &mut progress,
    )
}

// Deploys a release while reporting durable lifecycle milestones to the supplied callback.
pub(crate) fn deploy_release_with_progress(
    connection: &mut Connection,
    application_id: &ApplicationId,
    release: &Release,
    deployment_type: DeploymentType,
    source_revision: Option<&SourceRevision>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut dyn FnMut(DeploymentProgress),
) -> Result<DeploymentResult, DeployReleaseError> {
    let mut progress = ProgressReporter::enabled(progress);
    deploy_release_reporting(
        connection,
        application_id,
        release,
        deployment_type,
        source_revision,
        public_configuration,
        &mut progress,
    )
}

// Creates the durable deployment record before external effects, then routes failures through
// one finalizer that records failure and cleans up candidate resources.
fn deploy_release_reporting(
    connection: &mut Connection,
    application_id: &ApplicationId,
    release: &Release,
    deployment_type: DeploymentType,
    source_revision: Option<&SourceRevision>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeploymentResult, DeployReleaseError> {
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
    if specification.visibility == Visibility::Public && public_configuration.is_none() {
        return Err(DeployReleaseError::PublicApplication {
            application_id: application_id.to_string(),
        });
    }

    let database_path =
        connection
            .path()
            .map(std::path::Path::new)
            .ok_or(DeployReleaseError::OperationLock {
                source: ApplicationLockError::DatabasePathUnavailable,
            })?;
    let Some(_lock) = ApplicationLock::try_acquire(database_path, application_id)
        .map_err(|source| DeployReleaseError::OperationLock { source })?
    else {
        return Err(DeployReleaseError::OperationInProgress {
            application_id: application_id.to_string(),
        });
    };
    let owner_token = operation_store::generate_token(connection)
        .map_err(|source| DeployReleaseError::OperationToken { source })?;

    progress.started(
        DeploymentStep::CreateDeployment,
        format!("release {}", release.id),
    );
    let deployment = create_deployment_with_source_revision_and_ownership(
        connection,
        application_id,
        &release.id,
        deployment_type,
        source_revision,
        Some(&owner_token),
    )
    .map_err(|source| DeployReleaseError::CreateDeployment { source })?;
    progress.completed(
        DeploymentStep::CreateDeployment,
        format!("deployment {}", deployment.id),
    );
    progress.state_changed(deployment.id.as_str(), DeploymentStatus::Pending);

    let execution = execute_deployment(
        connection,
        &deployment.id,
        &specification,
        &release.artifact,
        public_configuration,
        progress,
    );
    match execution {
        Ok(execution) => Ok(DeploymentResult {
            deployment_id: deployment.id,
            runtime_id: execution.runtime_id,
            container_name: execution.container_name,
            artifact: release.artifact.clone(),
            source_revision: deployment.source_revision,
            finished_at: execution.finished_at,
        }),
        Err(failed) => finish_failed_deployment(connection, &deployment.id, failed, progress),
    }
}

// Loads the complete persisted specification needed to execute a deployment.
fn load_specification(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<ApplicationDeploymentSpecification, DeployReleaseError> {
    let spec = match application_store::load_deployment_specification(connection, application_id) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return Err(DeployReleaseError::ApplicationNotFound {
                application_id: application_id.to_string(),
            });
        }
        Err(ApplicationStoreError::NotFound { application_id }) => {
            return Err(DeployReleaseError::ApplicationNotFound { application_id });
        }
        Err(source) => {
            return Err(DeployReleaseError::LoadApplication { source });
        }
    };

    Ok(spec)
}

// Preserves failure provenance and every allocated candidate resource for ordered cleanup.
struct FailedExecution {
    code: &'static str,
    source: Box<dyn Error>,
    failure_persisted: bool,
    resources: CandidateResources,
}

impl FailedExecution {
    // Creates a failure whose stage still requires persistence before candidate cleanup.
    fn needing_persistence(
        code: &'static str,
        source: Box<dyn Error>,
        resources: CandidateResources,
    ) -> Self {
        Self {
            code,
            source,
            failure_persisted: false,
            resources,
        }
    }
}

// Keeps the three facts produced by candidate execution together until they form a result.
struct CompletedDeploymentExecution {
    runtime_id: RuntimeInstanceId,
    container_name: String,
    finished_at: String,
}

// Starts and verifies a candidate outside database transactions, promoting it only after the
// visibility-specific health checks succeed.
fn execute_deployment(
    connection: &mut Connection,
    deployment_id: &DeploymentId,
    specification: &ApplicationDeploymentSpecification,
    artifact: &OciArtifact,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<CompletedDeploymentExecution, FailedExecution> {
    wait_for_test_gate("deployment.pending")
        .map_err(|source| failure_needing_persistence("test_gate_failed", source, None, None))?;
    progress.state_changed(deployment_id.as_str(), DeploymentStatus::Starting);
    progress.started(
        DeploymentStep::CreateContainer,
        format!("image {}", artifact.reference()),
    );

    let input = CandidateStartInput {
        connection,
        deployment_id,
        application_id: &specification.application_id,
        application_name: &specification.application_name,
        artifact,
        runtime: &specification.runtime,
    };

    let candidate = start_candidate(input).map_err(candidate_start_failure)?;

    progress.completed(
        DeploymentStep::CreateContainer,
        format!(
            "unit {}, endpoint 127.0.0.1:{}",
            candidate.unit_name,
            candidate.port.get()
        ),
    );
    progress.completed(
        DeploymentStep::StartContainer,
        format!("container {}", candidate.runtime.external_runtime_id),
    );
    progress.completed(
        DeploymentStep::ObserveContainer,
        format!(
            "state Running, expected endpoint {}",
            candidate.runtime.expected_endpoint.socket_addr()
        ),
    );
    progress.completed(
        DeploymentStep::RegisterCandidate,
        format!("runtime {}", candidate.runtime.id),
    );
    wait_for_test_gate("deployment.starting-registered")
        .map_err(|source| started_candidate_failure("test_gate_failed", source, &candidate))?;
    progress.state_changed(deployment_id.as_str(), DeploymentStatus::Verifying);
    wait_for_test_gate("deployment.verifying")
        .map_err(|source| started_candidate_failure("test_gate_failed", source, &candidate))?;

    let previous_runtime = load_previous_runtime(
        connection,
        &specification.application_id,
        &candidate.runtime.id,
    )
    .map_err(|source| {
        started_candidate_failure("runtime_reconciliation_failed", source, &candidate)
    })?;

    let execution = match specification.visibility {
        Visibility::Public => finish_public_deployment(
            connection,
            specification,
            &candidate,
            public_configuration,
            progress,
        )?,
        Visibility::Internal => {
            finish_internal_deployment(connection, specification, &candidate, progress)?
        }
    };

    retire_previous_runtime(
        connection,
        &specification.application_name,
        previous_runtime.as_ref(),
    );

    Ok(execution)
}

// Completes a public deployment by exposing the verified candidate through Caddy and
// confirming its external route; the caller retires the previous runtime afterwards.
fn finish_public_deployment(
    connection: &mut Connection,
    specification: &ApplicationDeploymentSpecification,
    candidate: &StartedCandidate,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<CompletedDeploymentExecution, FailedExecution> {
    let Some(public_configuration) = public_configuration else {
        return Err(failure_needing_persistence(
            "public_configuration_missing",
            DeployReleaseError::PublicApplication {
                application_id: specification.application_id.to_string(),
            },
            Some(&candidate.runtime.external_runtime_id),
            Some(&candidate.runtime.id),
        ));
    };

    let input = PublicActivationInput {
        connection,
        runtime: &candidate.runtime,
        application_id: &specification.application_id,
        health_check: specification.runtime.health_check(),
        managed_caddy_directory: &public_configuration.managed_caddy_directory,
        caddyfile_path: &public_configuration.caddyfile_path,
    };
    let activated = activate_public_candidate(input, progress)
        .map_err(|error| public_activation_failure(error, &candidate.unit_name))?;

    Ok(CompletedDeploymentExecution {
        runtime_id: candidate.runtime.id.clone(),
        container_name: candidate.container_name.clone(),
        finished_at: activated.finished_at,
    })
}

// Completes an internal deployment by promoting the verified candidate to Current;
// the caller retires the previous runtime only after this promotion succeeds.
fn finish_internal_deployment(
    connection: &mut Connection,
    specification: &ApplicationDeploymentSpecification,
    candidate: &StartedCandidate,
    progress: &mut ProgressReporter<'_>,
) -> Result<CompletedDeploymentExecution, FailedExecution> {
    progress.started(
        DeploymentStep::HealthCheckAndPromotion,
        format!(
            "runtime {}, path {}, expected status {}",
            candidate.runtime.id,
            specification.runtime.health_check().path().as_str(),
            specification.runtime.health_check().expected_status().get()
        ),
    );
    let promoted = promote_internal_candidate(
        connection,
        &candidate.runtime.id,
        specification.runtime.health_check(),
    )
    .map_err(|error| {
        internal_promotion_failure(
            error,
            &candidate.runtime.external_runtime_id,
            &candidate.runtime.id,
            &candidate.unit_name,
        )
    })?;
    progress.completed(
        DeploymentStep::HealthCheckAndPromotion,
        format!("runtime {} promoted to Current", candidate.runtime.id),
    );
    progress.state_changed(
        candidate.runtime.deployment_id.as_str(),
        DeploymentStatus::Succeeded,
    );

    Ok(CompletedDeploymentExecution {
        runtime_id: candidate.runtime.id.clone(),
        container_name: candidate.container_name.clone(),
        finished_at: promoted.finished_at,
    })
}

// Finalizes a failed deployment: records its failure stage, releases candidate resources,
// and reports the highest-precedence recovery error so externally diverged state is never hidden.
fn finish_failed_deployment(
    connection: &mut Connection,
    deployment_id: &DeploymentId,
    failed: FailedExecution,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeploymentResult, DeployReleaseError> {
    let failure = failed.source.to_string();
    let record_error =
        persist_failure_if_needed(connection, deployment_id, &failed, &failure, progress);
    let resources = failed.resources;
    let cleanup_error = cleanup_candidate_if_needed(connection, deployment_id, resources, progress);

    Err(resolve_failure_recovery(
        deployment_id,
        failed.code,
        failed.source,
        failure,
        record_error,
        cleanup_error,
    ))
}

// Records the failure stage unless promotion already persisted it, reporting durable
// progress either way; persistence divergence is returned instead of being swallowed.
fn persist_failure_if_needed(
    connection: &mut Connection,
    deployment_id: &DeploymentId,
    failed: &FailedExecution,
    failure: &str,
    progress: &mut ProgressReporter<'_>,
) -> Option<TransitionDeploymentError> {
    if failed.failure_persisted {
        progress.failure_persisted(deployment_id.as_str(), failed.code);
        return None;
    }
    match fail_deployment(connection, deployment_id, failed.code, failure) {
        Ok(_) => {
            progress.failure_persisted(deployment_id.as_str(), failed.code);
            None
        }
        Err(source) => Some(source),
    }
}

// Releases every resource held by the failed candidate, reporting cleanup progress;
// cleanup divergence is returned so it can outrank the original failure.
fn cleanup_candidate_if_needed(
    connection: &Connection,
    deployment_id: &DeploymentId,
    resources: CandidateResources,
    progress: &mut ProgressReporter<'_>,
) -> Option<CandidateCleanupError> {
    if !resources.needs_cleanup() {
        return None;
    }
    progress.started(
        DeploymentStep::CleanupCandidate,
        format!("deployment {deployment_id}"),
    );
    match cleanup_failed_candidate(
        connection,
        deployment_id,
        resources.unit_name.as_deref(),
        resources.container_id.as_ref(),
        resources.runtime_id.as_ref(),
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
}

// Applies the established recovery precedence: cleanup divergence first, then failure
// recording divergence, and finally the original deployment failure itself.
fn resolve_failure_recovery(
    deployment_id: &DeploymentId,
    code: &'static str,
    source: Box<dyn Error>,
    failure: String,
    record_error: Option<TransitionDeploymentError>,
    cleanup_error: Option<CandidateCleanupError>,
) -> DeployReleaseError {
    if let Some(source) = cleanup_error {
        return DeployReleaseError::Cleanup {
            deployment_id: deployment_id.to_string(),
            failure,
            source: Box::new(source),
        };
    }
    if let Some(source) = record_error {
        return DeployReleaseError::RecordFailure {
            deployment_id: deployment_id.to_string(),
            failure,
            source,
        };
    }

    DeployReleaseError::DeploymentFailed {
        deployment_id: deployment_id.to_string(),
        code,
        source,
    }
}

// Git, build, runtime, and ordinary promotion errors do not update the deployment
// themselves. Tag them as needing persistence so the common finalizer records the
// correct failure stage before performing any candidate cleanup.
fn failure_needing_persistence(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&crate::domain::runtime::ContainerId>,
    runtime_id: Option<&RuntimeInstanceId>,
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

// Collects all resources allocated before a failure so the common finalizer can clean them up.
fn candidate_failure(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&crate::domain::runtime::ContainerId>,
    runtime_id: Option<&RuntimeInstanceId>,
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

// Tags a failure after full candidate startup so compensation retains every resource
// a started candidate holds: container, runtime, unit, and reserved port.
fn started_candidate_failure(
    code: &'static str,
    source: impl Error + 'static,
    candidate: &StartedCandidate,
) -> FailedExecution {
    candidate_failure(
        code,
        source,
        Some(&candidate.runtime.external_runtime_id),
        Some(&candidate.runtime.id),
        Some(&candidate.unit_name),
        true,
    )
}

// An unhealthy-candidate result is different: promotion persists `Failed` before it
// returns the error. Tag it as already persisted so the finalizer removes the rejected
// candidate without trying to fail an already-terminal deployment a second time.
fn failure_already_persisted(
    code: &'static str,
    source: impl Error + 'static,
    container_id: &crate::domain::runtime::ContainerId,
    runtime_id: &RuntimeInstanceId,
) -> FailedExecution {
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: true,
        resources: CandidateResources::with_container_and_runtime(container_id, runtime_id),
    }
}

// Maps candidate startup failures to their durable failure codes, retaining whatever
// resources each stage had already allocated for compensation.
fn candidate_start_failure(error: CandidateStartError) -> FailedExecution {
    match error {
        CandidateStartError::PortAllocation { source } => {
            failure_needing_persistence("runtime_port_allocation_failed", source, None, None)
        }
        CandidateStartError::UnitCreation { source, resources } => {
            FailedExecution::needing_persistence(
                "runtime_unit_creation_failed",
                Box::new(source),
                *resources,
            )
        }
        CandidateStartError::UnitReload { source, resources } => {
            FailedExecution::needing_persistence(
                "runtime_unit_reload_failed",
                Box::new(source),
                *resources,
            )
        }
        CandidateStartError::UnitStart { source, resources } => {
            FailedExecution::needing_persistence(
                "runtime_start_failed",
                Box::new(source),
                *resources,
            )
        }
        CandidateStartError::ContainerResolution { source, resources } => {
            FailedExecution::needing_persistence("runtime_resolution_failed", source, *resources)
        }
        CandidateStartError::ContainerObservation { source, resources } => {
            FailedExecution::needing_persistence("runtime_observation_failed", source, *resources)
        }
        CandidateStartError::RuntimeRegistration { source, resources } => {
            FailedExecution::needing_persistence("runtime_registration_failed", source, *resources)
        }
        CandidateStartError::PortPersistence { source, resources } => {
            FailedExecution::needing_persistence(
                "runtime_port_persistence_failed",
                Box::new(source),
                *resources,
            )
        }
        CandidateStartError::DeploymentTransition { source, resources } => {
            FailedExecution::needing_persistence(
                "deployment_transition_failed",
                Box::new(source),
                *resources,
            )
        }
    }
}

// Maps public activation failures to their durable failure codes. The activation input is
// a fully started candidate, so its unit and port are always part of the compensation set.
fn public_activation_failure(error: PublicActivationError, unit_name: &str) -> FailedExecution {
    let failed = match error {
        PublicActivationError::InternalHealth { source, resources } => {
            FailedExecution::needing_persistence("health_check_failed", source, *resources)
        }
        PublicActivationError::DeploymentTransition { source, resources } => {
            FailedExecution::needing_persistence(
                "deployment_transition_failed",
                Box::new(source),
                *resources,
            )
        }
        PublicActivationError::ExposurePreparation { source, resources } => {
            FailedExecution::needing_persistence("exposure_preparation_failed", source, *resources)
        }
        PublicActivationError::TestGate { source, resources } => {
            FailedExecution::needing_persistence("test_gate_failed", source, *resources)
        }
        PublicActivationError::CaddyMaterialization { source, resources } => {
            FailedExecution::needing_persistence("caddy_materialization_failed", source, *resources)
        }
        PublicActivationError::ExternalHealth { source, resources } => {
            FailedExecution::needing_persistence("external_health_check_failed", source, *resources)
        }
        PublicActivationError::PublicPromotion { source, resources } => {
            FailedExecution::needing_persistence("candidate_promotion_failed", source, *resources)
        }
    };
    FailedExecution {
        resources: failed.resources.with_unit(unit_name).with_port(),
        ..failed
    }
}

// Distinguishes an unhealthy candidate, whose rejection promotion already persisted as
// `Failed`, from other promotion errors; either way the started unit and port join the
// compensation set.
fn internal_promotion_failure(
    error: PromoteInternalCandidateError,
    container_id: &crate::domain::runtime::ContainerId,
    runtime_id: &RuntimeInstanceId,
    unit_name: &str,
) -> FailedExecution {
    let mut failed = if matches!(
        &error,
        PromoteInternalCandidateError::CandidateUnhealthy { .. }
    ) {
        failure_already_persisted("health_check_failed", error, container_id, runtime_id)
    } else {
        failure_needing_persistence(
            "candidate_promotion_failed",
            error,
            Some(container_id),
            Some(runtime_id),
        )
    };
    failed.resources = failed.resources.with_unit(unit_name).with_port();
    failed
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::super::candidate::CandidateStartError;
    use super::super::cleanup::{CandidateCleanupError, CandidateResources};
    use super::super::transition::TransitionDeploymentError;
    use super::{
        DeployReleaseError, candidate_start_failure, internal_promotion_failure,
        public_activation_failure, resolve_failure_recovery,
    };
    use crate::adapters::health_check_internal::{HealthCheckFailure, HealthCheckResult};
    use crate::adapters::port_allocator::PortAllocationError;
    use crate::adapters::systemd_quadlet::QuadletError;
    use crate::domain::identity::{DeploymentId, RuntimeInstanceId};
    use crate::domain::runtime::ContainerId;
    use crate::use_cases::deployment::activation::PublicActivationError;
    use crate::use_cases::deployment::promotion::PromoteInternalCandidateError;

    #[derive(Debug)]
    struct TestFailure;

    impl fmt::Display for TestFailure {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("test failure")
        }
    }

    impl std::error::Error for TestFailure {}

    fn container_id() -> ContainerId {
        ContainerId::from("abc123def456")
    }

    fn runtime_id() -> RuntimeInstanceId {
        RuntimeInstanceId::from("runtime-1")
    }

    fn deployment_id() -> DeploymentId {
        DeploymentId::from("deployment-1")
    }

    fn started_resources() -> CandidateResources {
        CandidateResources::with_container_and_runtime(&container_id(), &runtime_id())
    }

    fn transition_error() -> TransitionDeploymentError {
        TransitionDeploymentError::DeploymentNotFound {
            deployment_id: "deployment-1".to_owned(),
        }
    }

    #[test]
    fn candidate_start_failures_keep_their_stage_codes_and_resources() {
        let cases: Vec<(CandidateStartError, &'static str)> = vec![
            (
                CandidateStartError::PortAllocation {
                    source: PortAllocationError::InvalidRange {
                        value: "x".to_owned(),
                    },
                },
                "runtime_port_allocation_failed",
            ),
            (
                CandidateStartError::UnitCreation {
                    source: QuadletError::HomeUnavailable,
                    resources: Box::new(CandidateResources::empty().with_port()),
                },
                "runtime_unit_creation_failed",
            ),
            (
                CandidateStartError::UnitReload {
                    source: QuadletError::HomeUnavailable,
                    resources: Box::new(CandidateResources::empty().with_port()),
                },
                "runtime_unit_reload_failed",
            ),
            (
                CandidateStartError::UnitStart {
                    source: QuadletError::HomeUnavailable,
                    resources: Box::new(CandidateResources::empty().with_port()),
                },
                "runtime_start_failed",
            ),
            (
                CandidateStartError::ContainerResolution {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                "runtime_resolution_failed",
            ),
            (
                CandidateStartError::ContainerObservation {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                "runtime_observation_failed",
            ),
            (
                CandidateStartError::RuntimeRegistration {
                    source: Box::new(TestFailure),
                    resources: Box::new(CandidateResources::with_container(&container_id())),
                },
                "runtime_registration_failed",
            ),
            (
                CandidateStartError::PortPersistence {
                    source: PortAllocationError::InvalidRange {
                        value: "x".to_owned(),
                    },
                    resources: Box::new(started_resources()),
                },
                "runtime_port_persistence_failed",
            ),
            (
                CandidateStartError::DeploymentTransition {
                    source: transition_error(),
                    resources: Box::new(started_resources()),
                },
                "deployment_transition_failed",
            ),
        ];

        // Port allocation fails before anything is allocated, so only later stages
        // retain resources for compensation.
        let mut cases = cases;
        for (error, expected_code) in cases.drain(..) {
            let failed = candidate_start_failure(error);
            assert_eq!(failed.code, expected_code);
            assert!(!failed.failure_persisted);
            if expected_code == "runtime_port_allocation_failed" {
                assert!(
                    !failed.resources.needs_cleanup(),
                    "port allocation failures hold nothing to clean up"
                );
            } else {
                assert!(
                    failed.resources.needs_cleanup(),
                    "{expected_code} must retain resources for cleanup"
                );
            }
        }
    }

    #[test]
    fn public_activation_failures_keep_their_stage_codes_and_add_the_started_unit_and_port() {
        let cases: Vec<(PublicActivationError, &'static str)> = vec![
            (
                PublicActivationError::InternalHealth {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                "health_check_failed",
            ),
            (
                PublicActivationError::DeploymentTransition {
                    source: transition_error(),
                    resources: Box::new(started_resources()),
                },
                "deployment_transition_failed",
            ),
            (
                PublicActivationError::ExposurePreparation {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                "exposure_preparation_failed",
            ),
            (
                PublicActivationError::TestGate {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                "test_gate_failed",
            ),
            (
                PublicActivationError::CaddyMaterialization {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                "caddy_materialization_failed",
            ),
            (
                PublicActivationError::ExternalHealth {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                "external_health_check_failed",
            ),
            (
                PublicActivationError::PublicPromotion {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                "candidate_promotion_failed",
            ),
        ];

        for (error, expected_code) in cases {
            let failed = public_activation_failure(error, "unit-1");
            assert_eq!(failed.code, expected_code);
            assert!(!failed.failure_persisted);
            // Activation starts from a registered candidate, so its container and runtime
            // are already tracked and the started unit and port must be added.
            assert!(failed.resources.container_id.is_some());
            assert!(failed.resources.runtime_id.is_some());
            assert_eq!(failed.resources.unit_name.as_deref(), Some("unit-1"));
            assert!(failed.resources.port_reserved);
        }
    }

    #[test]
    fn internal_promotion_failure_is_already_persisted_for_unhealthy_candidates() {
        let failed = internal_promotion_failure(
            PromoteInternalCandidateError::CandidateUnhealthy {
                result: HealthCheckResult::Unhealthy {
                    attempts: 1,
                    failure: HealthCheckFailure::TimedOut,
                },
            },
            &container_id(),
            &runtime_id(),
            "unit-1",
        );

        assert_eq!(failed.code, "health_check_failed");
        assert!(failed.failure_persisted);
        assert_eq!(failed.resources.unit_name.as_deref(), Some("unit-1"));
        assert!(failed.resources.port_reserved);
    }

    #[test]
    fn internal_promotion_failure_needs_persistence_for_other_promotion_errors() {
        let failed = internal_promotion_failure(
            PromoteInternalCandidateError::RuntimeNotFound {
                runtime_id: "runtime-9".to_owned(),
            },
            &container_id(),
            &runtime_id(),
            "unit-1",
        );

        assert_eq!(failed.code, "candidate_promotion_failed");
        assert!(!failed.failure_persisted);
        assert_eq!(failed.resources.unit_name.as_deref(), Some("unit-1"));
        assert!(failed.resources.port_reserved);
    }

    #[test]
    fn cleanup_divergence_outranks_failure_recording_divergence() {
        let error = resolve_failure_recovery(
            &deployment_id(),
            "runtime_start_failed",
            Box::new(TestFailure),
            "test failure".to_owned(),
            Some(transition_error()),
            Some(CandidateCleanupError::RuntimeChanged {
                runtime_id: runtime_id(),
            }),
        );

        match error {
            DeployReleaseError::Cleanup { failure, .. } => {
                assert_eq!(failure, "test failure");
            }
            other => panic!("expected the cleanup divergence to win, got {other:?}"),
        }
    }

    #[test]
    fn failure_recording_divergence_outranks_the_original_failure() {
        let error = resolve_failure_recovery(
            &deployment_id(),
            "runtime_start_failed",
            Box::new(TestFailure),
            "test failure".to_owned(),
            Some(transition_error()),
            None,
        );

        assert!(matches!(error, DeployReleaseError::RecordFailure { .. }));
    }

    #[test]
    fn without_recovery_divergence_the_original_failure_wins() {
        let error = resolve_failure_recovery(
            &deployment_id(),
            "runtime_start_failed",
            Box::new(TestFailure),
            "test failure".to_owned(),
            None,
            None,
        );

        match error {
            DeployReleaseError::DeploymentFailed { code, .. } => {
                assert_eq!(code, "runtime_start_failed");
            }
            other => panic!("expected the original failure, got {other:?}"),
        }
    }
}
