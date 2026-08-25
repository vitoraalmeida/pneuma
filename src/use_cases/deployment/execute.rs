use std::path::PathBuf;

use rusqlite::Connection;

use super::activation::{PublicActivationInput, activate_public_candidate};
use super::candidate::{CandidateStartInput, StartedCandidate, start_candidate};
use super::cleanup::{load_previous_runtime, retire_previous_runtime};
use super::create::create_deployment_with_source_revision_and_ownership;
use super::failure::{
    DeployReleaseError, FailedExecution, candidate_start_failure, failure_needing_persistence,
    finish_failed_deployment, internal_promotion_failure, public_activation_failure,
    started_candidate_failure,
};
use super::progress::{DeploymentStep, ProgressReporter};
use super::promotion::promote_internal_candidate;
use crate::adapters::application_lock::{ApplicationLock, ApplicationLockError};
use crate::adapters::stores::application_store;
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

// Creates the durable deployment record before external effects, then routes failures through
// one finalizer that records failure and cleans up candidate resources. Callers supply the
// reporter they want: disabled for silent execution, enabled for lifecycle milestones.
pub(crate) fn deploy_release_reporting(
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
        Err(failed) => Err(finish_failed_deployment(
            connection,
            &deployment.id,
            failed,
            progress,
        )),
    }
}

// Loads the complete persisted specification needed to execute a deployment.
fn load_specification(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<ApplicationDeploymentSpecification, DeployReleaseError> {
    application_store::load_deployment_specification(connection, application_id)
        .map_err(|source| DeployReleaseError::LoadApplication { source })?
        .ok_or_else(|| DeployReleaseError::ApplicationNotFound {
            application_id: application_id.to_string(),
        })
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
    let health_check = specification.runtime.health_check();
    progress.started(
        DeploymentStep::HealthCheckAndPromotion,
        format!(
            "runtime {}, path {}, expected status {}",
            candidate.runtime.id,
            health_check.path().as_str(),
            health_check.expected_status().get()
        ),
    );
    let promoted = promote_internal_candidate(connection, &candidate.runtime.id, health_check)
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
