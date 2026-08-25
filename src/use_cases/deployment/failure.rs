//! Deployment failure vocabulary: how a failed execution is represented and classified,
//! how its failure stage is persisted, when candidate resources are released, and how the
//! final workflow error is chosen.
//!
//! `execute` owns the success narrative and hands any `FailedExecution` here; this module
//! owns every decision about what happens to a deployment failure afterwards.

use std::error::Error;

use rusqlite::Connection;
use thiserror::Error;

use super::activation::PublicActivationError;
use super::candidate::{CandidateStartError, StartedCandidate};
use super::cleanup::{CandidateCleanupError, CandidateResources, cleanup_failed_candidate};
use super::create::CreateDeploymentError;
use super::progress::{DeploymentStep, ProgressReporter};
use super::promotion::PromoteInternalCandidateError;
use super::transition::{TransitionDeploymentError, fail_deployment};
use crate::adapters::application_lock::ApplicationLockError;
use crate::adapters::stores::application_store::ApplicationStoreError;
use crate::adapters::stores::operation_store::OperationStoreError;
use crate::domain::identity::{DeploymentId, RuntimeInstanceId};
use crate::domain::runtime::ContainerId;

#[derive(Debug, Error)]
pub enum DeployReleaseError {
    #[error("application `{application_id}` was not found")]
    ApplicationNotFound { application_id: String },
    #[error("application `{application_id}` requires public deployment support")]
    PublicApplication { application_id: String },
    #[error("failed to load deployment specification: {source}")]
    LoadApplication {
        #[source]
        source: ApplicationStoreError,
    },
    #[error("{source}")]
    CreateDeployment {
        #[source]
        source: CreateDeploymentError,
    },
    #[error("deployment `{deployment_id}` failed with `{code}`: {source}")]
    DeploymentFailed {
        deployment_id: String,
        code: &'static str,
        #[source]
        source: Box<dyn Error>,
    },
    #[error(
        "deployment `{deployment_id}` encountered `{failure}` and its failure could not be recorded: {source}"
    )]
    RecordFailure {
        deployment_id: String,
        failure: String,
        #[source]
        source: TransitionDeploymentError,
    },
    #[error(
        "deployment `{deployment_id}` encountered `{failure}` and its candidate could not be cleaned up: {source}"
    )]
    Cleanup {
        deployment_id: String,
        failure: String,
        #[source]
        source: Box<CandidateCleanupError>,
    },
    #[error("failed to serialize deployment: {source}")]
    OperationLock {
        #[source]
        source: ApplicationLockError,
    },
    #[error("failed to create deployment ownership: {source}")]
    OperationToken {
        #[source]
        source: OperationStoreError,
    },
    #[error("application `{application_id}` already has an operation in progress")]
    OperationInProgress { application_id: String },
}

// Preserves failure provenance and every allocated candidate resource for ordered cleanup.
pub(crate) struct FailedExecution {
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

    // Creates a failure whose step already persisted the terminal stage before returning.
    fn already_persisted(
        code: &'static str,
        source: impl Error + 'static,
        container_id: &ContainerId,
        runtime_id: &RuntimeInstanceId,
    ) -> Self {
        Self {
            code,
            source: Box::new(source),
            failure_persisted: true,
            resources: CandidateResources::with_container_and_runtime(container_id, runtime_id),
        }
    }
}

// Finalizes a failed deployment: records its failure stage, releases candidate resources,
// and reports the highest-precedence recovery error so externally diverged state is never hidden.
pub(crate) fn finish_failed_deployment(
    connection: &mut Connection,
    deployment_id: &DeploymentId,
    failed: FailedExecution,
    progress: &mut ProgressReporter<'_>,
) -> DeployReleaseError {
    let failure = failed.source.to_string();
    let record_error =
        persist_failure_if_needed(connection, deployment_id, &failed, &failure, progress);
    let resources = failed.resources;
    let cleanup_error = cleanup_candidate_if_needed(connection, deployment_id, resources, progress);

    resolve_failure_recovery(
        deployment_id,
        failed.code,
        failed.source,
        failure,
        record_error,
        cleanup_error,
    )
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
pub(crate) fn failure_needing_persistence(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&ContainerId>,
    runtime_id: Option<&RuntimeInstanceId>,
) -> FailedExecution {
    FailedExecution::needing_persistence(
        code,
        Box::new(source),
        candidate_resources(container_id, runtime_id),
    )
}

// Collects whatever a candidate allocated so far from its optional tracking identifiers.
fn candidate_resources(
    container_id: Option<&ContainerId>,
    runtime_id: Option<&RuntimeInstanceId>,
) -> CandidateResources {
    match (container_id, runtime_id) {
        (Some(cid), Some(rid)) => CandidateResources::with_container_and_runtime(cid, rid),
        (Some(cid), None) => CandidateResources::with_container(cid),
        _ => CandidateResources::empty(),
    }
}

// Collects all resources allocated before a failure so the common finalizer can clean them up.
fn candidate_failure(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&ContainerId>,
    runtime_id: Option<&RuntimeInstanceId>,
    unit_name: Option<&str>,
    port_reserved: bool,
) -> FailedExecution {
    let mut failed = failure_needing_persistence(code, source, container_id, runtime_id);
    if let Some(unit) = unit_name {
        failed.resources = failed.resources.with_unit(unit);
    }
    if port_reserved {
        failed.resources = failed.resources.with_port();
    }
    failed
}

// Tags a failure after full candidate startup so compensation retains every resource
// a started candidate holds: container, runtime, unit, and reserved port.
pub(crate) fn started_candidate_failure(
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

// Maps candidate startup failures to their durable failure codes, retaining whatever
// resources each stage had already allocated for compensation.
pub(crate) fn candidate_start_failure(error: CandidateStartError) -> FailedExecution {
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
pub(crate) fn public_activation_failure(
    error: PublicActivationError,
    unit_name: &str,
) -> FailedExecution {
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
pub(crate) fn internal_promotion_failure(
    error: PromoteInternalCandidateError,
    container_id: &ContainerId,
    runtime_id: &RuntimeInstanceId,
    unit_name: &str,
) -> FailedExecution {
    let mut failed = if matches!(
        &error,
        PromoteInternalCandidateError::CandidateUnhealthy { .. }
    ) {
        FailedExecution::already_persisted("health_check_failed", error, container_id, runtime_id)
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
