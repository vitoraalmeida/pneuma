//! Deployment failure vocabulary: how a failed execution is represented and classified,
//! how its failure stage is persisted, when candidate resources are released, and how the
//! final workflow error is chosen.
//!
//! `execute` owns the success narrative and hands any `FailedExecution` here; this module
//! owns every decision about what happens to a deployment failure afterwards.

use std::error::Error;

use rusqlite::Connection;
use thiserror::Error;

use super::cleanup::{CandidateCleanupError, CandidateResources, cleanup_failed_candidate};
use super::create::CreateDeploymentError;
use super::progress::{DeploymentStep, ProgressReporter};
use super::promotion::PromoteInternalCandidateError;
use super::transition::{TransitionDeploymentError, fail_deployment};
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
        code: DeploymentFailureCode,
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
    pub(crate) fn needing_persistence(
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
    pub(crate) fn with_started_unit(mut self, unit_name: &str) -> Self {
        self.resources = self.resources.with_unit(unit_name).with_port();
        self
    }

    #[cfg(test)]
    pub(crate) fn code(&self) -> DeploymentFailureCode {
        self.code
    }

    #[cfg(test)]
    pub(crate) fn failure_persisted(&self) -> bool {
        self.failure_persisted
    }

    #[cfg(test)]
    pub(crate) fn resources(&self) -> &CandidateResources {
        &self.resources
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
    match fail_deployment(connection, deployment_id, failed.code, failure) {
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
        code,
        source,
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

    use super::super::cleanup::{CandidateCleanupError, CandidateResources};
    use super::super::transition::TransitionDeploymentError;
    use super::{
        DeployReleaseError, DeploymentFailureCode, FailedExecution, ProgressReporter,
        internal_promotion_failure, persist_failure_if_needed, resolve_failure_recovery,
    };
    use crate::adapters::health_check_internal::{HealthCheckFailure, HealthCheckResult};
    use crate::domain::identity::{DeploymentId, RuntimeInstanceId};
    use crate::domain::runtime::ContainerId;
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
        RuntimeInstanceId::new("11111111111111111111111111111111").unwrap()
    }

    fn deployment_id() -> DeploymentId {
        DeploymentId::new("22222222222222222222222222222222").unwrap()
    }

    fn transition_error() -> TransitionDeploymentError {
        TransitionDeploymentError::DeploymentNotFound {
            deployment_id: "deployment-1".to_owned(),
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
                assert_eq!(code, DeploymentFailureCode::RuntimeStart);
            }
            other => panic!("expected the original failure, got {other:?}"),
        }
    }

    #[test]
    fn deployment_failed_keeps_the_semantic_code_typed_until_presentation() {
        let error = resolve_failure_recovery(
            &deployment_id(),
            DeploymentFailureCode::RuntimeStart,
            Box::new(TestFailure),
            "test failure".to_owned(),
            None,
            None,
        );

        let DeployReleaseError::DeploymentFailed {
            deployment_id,
            code,
            ..
        } = error
        else {
            panic!("expected the original failure, got {error:?}")
        };
        // The semantic code stays an enum, so callers never reparse the
        // persisted string, and the rendered text remains the stable one.
        assert_eq!(code, DeploymentFailureCode::RuntimeStart);
        assert_eq!(code.as_str(), "runtime_start_failed");
        assert_eq!(deployment_id, "22222222222222222222222222222222");
    }

    #[test]
    fn deployment_failed_formats_the_stable_persisted_code_string() {
        let error = DeployReleaseError::DeploymentFailed {
            deployment_id: "deployment-9".to_owned(),
            code: DeploymentFailureCode::HealthCheck,
            source: Box::new(TestFailure),
        };

        assert_eq!(
            error.to_string(),
            "deployment `deployment-9` failed with `health_check_failed`: test failure"
        );
    }

    #[test]
    fn already_persisted_failures_are_not_recorded_a_second_time() {
        let mut connection =
            crate::adapters::database::open(std::path::Path::new(":memory:")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO systems (id, name, created_at) VALUES ('44444444444444444444444444444444', 'team', 'now');
                 INSERT INTO applications (
                    id, system_id, name, desired_runtime_state, created_at, updated_at
                 ) VALUES ('11111111111111111111111111111111', '44444444444444444444444444444444', 'app', 'stopped', 'now', 'now');
                 INSERT INTO releases (
                    id, application_id, image_repository, image_digest, image_reference, created_at
                 ) VALUES (
                    '55555555555555555555555555555555', '11111111111111111111111111111111', 'registry.example/app',
                    'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'now'
                 );
                 INSERT INTO deployments (
                    id, application_id, release_id, type, status, requested_at
                 ) VALUES ('22222222222222222222222222222222', '11111111111111111111111111111111', '55555555555555555555555555555555', 'deploy', 'starting', 'now');",
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
                "SELECT status, failure_code FROM deployments WHERE id = '22222222222222222222222222222222'",
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
                "SELECT status, failure_code FROM deployments WHERE id = '22222222222222222222222222222222'",
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
