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
use crate::domain::deployment::DeploymentFailureCode;
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
    #[error(transparent)]
    CreateDeployment { source: CreateDeploymentError },
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
        source: rusqlite::Error,
    },
    #[error("application `{application_id}` already has an operation in progress")]
    OperationInProgress { application_id: String },
}

// Preserves failure provenance and every allocated candidate resource for ordered cleanup.
pub(crate) struct FailedExecution {
    code: DeploymentFailureCode,
    source: Box<dyn Error>,
    failure_persisted: bool,
    resources: CandidateResources,
}

impl FailedExecution {
    // Creates a failure whose stage still requires persistence before candidate cleanup.
    fn needing_persistence(
        code: DeploymentFailureCode,
        source: impl Into<Box<dyn Error>>,
        resources: CandidateResources,
    ) -> Self {
        Self {
            code,
            source: source.into(),
            failure_persisted: false,
            resources,
        }
    }

    // A started candidate always owns its unit and reserved port; both join compensation.
    fn with_started_unit(mut self, unit_name: &str) -> Self {
        self.resources = self.resources.with_unit(unit_name).with_port();
        self
    }

    // Creates a failure whose step already persisted the terminal stage before returning.
    fn already_persisted(
        code: DeploymentFailureCode,
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
        progress.failure_persisted(deployment_id.as_str(), failed.code.as_str());
        return None;
    }
    match fail_deployment(connection, deployment_id, failed.code.as_str(), failure) {
        Ok(_) => {
            progress.failure_persisted(deployment_id.as_str(), failed.code.as_str());
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
    code: DeploymentFailureCode,
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
        code: code.as_str(),
        source,
    }
}

// Git, build, runtime, and ordinary promotion errors do not update the deployment
// themselves. Tag them as needing persistence so the common finalizer records the
// correct failure stage before performing any candidate cleanup.
pub(crate) fn failure_needing_persistence(
    code: DeploymentFailureCode,
    source: impl Error + 'static,
    container_id: Option<&ContainerId>,
    runtime_id: Option<&RuntimeInstanceId>,
) -> FailedExecution {
    FailedExecution::needing_persistence(
        code,
        source,
        candidate_resources(container_id, runtime_id),
    )
}

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

// Tags a failure after full candidate startup so compensation retains every resource
// a started candidate holds: container, runtime, unit, and reserved port.
pub(crate) fn started_candidate_failure(
    code: DeploymentFailureCode,
    source: impl Error + 'static,
    candidate: &StartedCandidate,
) -> FailedExecution {
    FailedExecution::needing_persistence(
        code,
        source,
        CandidateResources::with_container_and_runtime(
            &candidate.runtime.external_runtime_id,
            &candidate.runtime.id,
        ),
    )
    .with_started_unit(&candidate.unit_name)
}

// Maps candidate startup failures to their durable failure codes, retaining whatever
// resources each stage had already allocated for compensation.
pub(crate) fn candidate_start_failure(error: CandidateStartError) -> FailedExecution {
    match error {
        CandidateStartError::PortAllocation { source } => FailedExecution::needing_persistence(
            DeploymentFailureCode::RuntimePortAllocation,
            source,
            CandidateResources::empty(),
        ),
        CandidateStartError::UnitCreation { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::RuntimeUnitCreation,
                source,
                *resources,
            )
        }
        CandidateStartError::UnitReload { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::RuntimeUnitReload,
                source,
                *resources,
            )
        }
        CandidateStartError::UnitStart { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::RuntimeStart,
                source,
                *resources,
            )
        }
        CandidateStartError::ContainerResolution { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::RuntimeResolution,
                source,
                *resources,
            )
        }
        CandidateStartError::ContainerObservation { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::RuntimeObservation,
                source,
                *resources,
            )
        }
        CandidateStartError::RuntimeRegistration { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::RuntimeRegistration,
                source,
                *resources,
            )
        }
        CandidateStartError::PortPersistence { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::RuntimePortPersistence,
                source,
                *resources,
            )
        }
        CandidateStartError::DeploymentTransition { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::DeploymentTransition,
                source,
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
            FailedExecution::needing_persistence(
                DeploymentFailureCode::HealthCheck,
                source,
                *resources,
            )
        }
        PublicActivationError::DeploymentTransition { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::DeploymentTransition,
                source,
                *resources,
            )
        }
        PublicActivationError::ExposurePreparation { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::ExposurePreparation,
                source,
                *resources,
            )
        }
        PublicActivationError::TestGate { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::TestGate,
                source,
                *resources,
            )
        }
        PublicActivationError::CaddyMaterialization { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::CaddyMaterialization,
                source,
                *resources,
            )
        }
        PublicActivationError::ExternalHealth { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::ExternalHealthCheck,
                source,
                *resources,
            )
        }
        PublicActivationError::PublicPromotion { source, resources } => {
            FailedExecution::needing_persistence(
                DeploymentFailureCode::CandidatePromotion,
                source,
                *resources,
            )
        }
    };
    failed.with_started_unit(unit_name)
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
    let failed = if matches!(
        &error,
        PromoteInternalCandidateError::CandidateUnhealthy { .. }
    ) {
        FailedExecution::already_persisted(
            DeploymentFailureCode::HealthCheck,
            error,
            container_id,
            runtime_id,
        )
    } else {
        FailedExecution::needing_persistence(
            DeploymentFailureCode::CandidatePromotion,
            error,
            CandidateResources::with_container_and_runtime(container_id, runtime_id),
        )
    };
    failed.with_started_unit(unit_name)
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::fmt;

    use super::super::candidate::CandidateStartError;
    use super::super::cleanup::{CandidateCleanupError, CandidateResources};
    use super::super::transition::TransitionDeploymentError;
    use super::{
        DeployReleaseError, DeploymentFailureCode, FailedExecution, ProgressReporter,
        candidate_start_failure, internal_promotion_failure, persist_failure_if_needed,
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

    // Asserts the exact compensation payload a stage must carry: which resource kinds
    // were already allocated when that stage failed.
    fn assert_resources(
        resources: &CandidateResources,
        port_reserved: bool,
        unit: bool,
        container: bool,
        runtime: bool,
    ) {
        assert_eq!(resources.port_reserved, port_reserved);
        assert_eq!(resources.unit_name.is_some(), unit);
        assert_eq!(resources.container_id.is_some(), container);
        assert_eq!(resources.runtime_id.is_some(), runtime);
    }

    #[test]
    fn candidate_start_failures_keep_their_stage_codes_and_resources() {
        // Each case records the exact compensation payload start_candidate attaches at
        // that stage; the mapping must carry it verbatim without dropping or adding.
        type StageCase = (
            CandidateStartError,
            DeploymentFailureCode,
            fn(&CandidateResources),
        );
        let cases: Vec<StageCase> = vec![
            (
                CandidateStartError::PortAllocation {
                    source: PortAllocationError::InvalidRange {
                        value: "x".to_owned(),
                    },
                },
                DeploymentFailureCode::RuntimePortAllocation,
                |resources| assert_resources(resources, false, false, false, false),
            ),
            (
                CandidateStartError::UnitCreation {
                    source: QuadletError::HomeUnavailable,
                    resources: Box::new(CandidateResources::empty().with_port()),
                },
                DeploymentFailureCode::RuntimeUnitCreation,
                |resources| assert_resources(resources, true, false, false, false),
            ),
            (
                CandidateStartError::UnitReload {
                    source: QuadletError::HomeUnavailable,
                    resources: Box::new(CandidateResources::empty().with_port()),
                },
                DeploymentFailureCode::RuntimeUnitReload,
                |resources| assert_resources(resources, true, false, false, false),
            ),
            (
                CandidateStartError::UnitStart {
                    source: QuadletError::HomeUnavailable,
                    resources: Box::new(CandidateResources::empty().with_port()),
                },
                DeploymentFailureCode::RuntimeStart,
                |resources| assert_resources(resources, true, false, false, false),
            ),
            (
                CandidateStartError::ContainerResolution {
                    source: Box::new(TestFailure),
                    resources: Box::new(
                        CandidateResources::empty().with_port().with_unit("unit-1"),
                    ),
                },
                DeploymentFailureCode::RuntimeResolution,
                |resources| assert_resources(resources, true, true, false, false),
            ),
            (
                CandidateStartError::ContainerObservation {
                    source: Box::new(TestFailure),
                    resources: Box::new(
                        CandidateResources::with_container(&container_id())
                            .with_port()
                            .with_unit("unit-1"),
                    ),
                },
                DeploymentFailureCode::RuntimeObservation,
                |resources| assert_resources(resources, true, true, true, false),
            ),
            (
                CandidateStartError::RuntimeRegistration {
                    source: Box::new(TestFailure),
                    resources: Box::new(CandidateResources::with_container(&container_id())),
                },
                DeploymentFailureCode::RuntimeRegistration,
                |resources| assert_resources(resources, false, false, true, false),
            ),
            (
                CandidateStartError::PortPersistence {
                    source: PortAllocationError::InvalidRange {
                        value: "x".to_owned(),
                    },
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::RuntimePortPersistence,
                |resources| assert_resources(resources, false, false, true, true),
            ),
            (
                CandidateStartError::DeploymentTransition {
                    source: transition_error(),
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::DeploymentTransition,
                |resources| assert_resources(resources, false, false, true, true),
            ),
        ];

        for (error, expected_code, expected_resources) in cases {
            let failed = candidate_start_failure(error);
            assert_eq!(failed.code, expected_code);
            assert!(!failed.failure_persisted);
            expected_resources(&failed.resources);
        }
    }

    #[test]
    fn public_activation_failures_keep_their_stage_codes_and_add_the_started_unit_and_port() {
        let cases: Vec<(PublicActivationError, DeploymentFailureCode)> = vec![
            (
                PublicActivationError::InternalHealth {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::HealthCheck,
            ),
            (
                PublicActivationError::DeploymentTransition {
                    source: transition_error(),
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::DeploymentTransition,
            ),
            (
                PublicActivationError::ExposurePreparation {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::ExposurePreparation,
            ),
            (
                PublicActivationError::TestGate {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::TestGate,
            ),
            (
                PublicActivationError::CaddyMaterialization {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::CaddyMaterialization,
            ),
            (
                PublicActivationError::ExternalHealth {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::ExternalHealthCheck,
            ),
            (
                PublicActivationError::PublicPromotion {
                    source: Box::new(TestFailure),
                    resources: Box::new(started_resources()),
                },
                DeploymentFailureCode::CandidatePromotion,
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

        assert_eq!(failed.code, DeploymentFailureCode::HealthCheck);
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

        assert_eq!(failed.code, DeploymentFailureCode::CandidatePromotion);
        assert!(!failed.failure_persisted);
        assert_eq!(failed.resources.unit_name.as_deref(), Some("unit-1"));
        assert!(failed.resources.port_reserved);
    }

    #[test]
    fn cleanup_divergence_outranks_failure_recording_divergence() {
        let error = resolve_failure_recovery(
            &deployment_id(),
            DeploymentFailureCode::RuntimeStart,
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
            DeploymentFailureCode::RuntimeStart,
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
            DeploymentFailureCode::RuntimeStart,
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

    #[test]
    fn already_persisted_failures_are_not_recorded_a_second_time() {
        let mut connection =
            crate::adapters::database::open(std::path::Path::new(":memory:")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO applications (
                    id, name, desired_runtime_state, spec_version, created_at, updated_at
                 ) VALUES ('app-1', 'app', 'stopped', 1, 'now', 'now');
                 INSERT INTO releases (
                    id, application_id, image_repository, image_digest, image_reference, created_at
                 ) VALUES (
                    'release-1', 'app-1', 'registry.example/app',
                    'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'now'
                 );
                 INSERT INTO deployments (
                    id, application_id, release_id, type, status, requested_at
                 ) VALUES ('deployment-1', 'app-1', 'release-1', 'deploy', 'starting', 'now');",
            )
            .unwrap();

        let mut failed = FailedExecution {
            code: DeploymentFailureCode::RuntimeStart,
            source: Box::new(TestFailure),
            failure_persisted: true,
            resources: CandidateResources::empty(),
        };
        let progress = &mut ProgressReporter::disabled();

        // A failure whose stage was already persisted must leave the deployment row alone.
        let recorded = persist_failure_if_needed(
            &mut connection,
            &deployment_id(),
            &failed,
            "test failure",
            progress,
        );
        assert!(recorded.is_none(), "no recording divergence is expected");
        let (status, failure_code): (String, Option<String>) = connection
            .query_row(
                "SELECT status, failure_code FROM deployments WHERE id = 'deployment-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "starting");
        assert_eq!(failure_code, None);

        // The same failure needing persistence is recorded exactly once.
        failed.failure_persisted = false;
        let recorded = persist_failure_if_needed(
            &mut connection,
            &deployment_id(),
            &failed,
            "test failure",
            progress,
        );
        assert!(recorded.is_none(), "no recording divergence is expected");
        let (status, failure_code): (String, Option<String>) = connection
            .query_row(
                "SELECT status, failure_code FROM deployments WHERE id = 'deployment-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(failure_code.as_deref(), Some("runtime_start_failed"));
    }

    #[test]
    fn deployment_failure_errors_expose_their_original_cause() {
        let original = resolve_failure_recovery(
            &deployment_id(),
            DeploymentFailureCode::RuntimeStart,
            Box::new(TestFailure),
            "test failure".to_owned(),
            None,
            None,
        );
        let source = original
            .source()
            .expect("DeploymentFailed must keep its cause");
        assert!(source.downcast_ref::<TestFailure>().is_some());

        let record_failure = resolve_failure_recovery(
            &deployment_id(),
            DeploymentFailureCode::RuntimeStart,
            Box::new(TestFailure),
            "test failure".to_owned(),
            Some(transition_error()),
            None,
        );
        let source = record_failure
            .source()
            .expect("RecordFailure must keep its cause");
        assert!(
            source
                .downcast_ref::<TransitionDeploymentError>()
                .is_some_and(|error| matches!(
                    error,
                    TransitionDeploymentError::DeploymentNotFound { .. }
                ))
        );

        let cleanup = resolve_failure_recovery(
            &deployment_id(),
            DeploymentFailureCode::RuntimeStart,
            Box::new(TestFailure),
            "test failure".to_owned(),
            None,
            Some(CandidateCleanupError::RuntimeChanged {
                runtime_id: runtime_id(),
            }),
        );
        let source = cleanup.source().expect("Cleanup must keep its cause");
        // The cleanup cause is stored boxed, so the first `source()` layer is the box
        // itself; the chain still reaches the cleanup error through it.
        assert!(
            source
                .downcast_ref::<Box<CandidateCleanupError>>()
                .is_some_and(|error| matches!(
                    error.as_ref(),
                    CandidateCleanupError::RuntimeChanged { .. }
                ))
        );
    }
}
