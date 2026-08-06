use std::path::{Path, PathBuf};

use pneuma::create_deployment::{DeploymentStatus, create_deployment};
use pneuma::database;
use pneuma::import_application::import_application;
use pneuma::transition_deployment::{
    DeploymentTransition, TransitionDeploymentError, advance_deployment, fail_deployment,
};

#[test]
fn advances_in_order_through_internal_verification() {
    let (mut connection, deployment_id, _) = pending_deployment();

    assert_eq!(
        advance_deployment(&connection, &deployment_id, DeploymentTransition::Start).unwrap(),
        DeploymentStatus::PreparingSource
    );
    let started_at: String = connection
        .query_row(
            "SELECT started_at FROM deployments WHERE id = ?1",
            [&deployment_id],
            |row| row.get(0),
        )
        .unwrap();
    for (transition, expected_status) in [
        (
            DeploymentTransition::SourcePrepared,
            DeploymentStatus::Building,
        ),
        (DeploymentTransition::ImageBuilt, DeploymentStatus::Starting),
        (
            DeploymentTransition::RuntimeRunning,
            DeploymentStatus::VerifyingInternal,
        ),
    ] {
        assert_eq!(
            advance_deployment(&connection, &deployment_id, transition).unwrap(),
            expected_status
        );
    }

    let timestamps = connection
        .query_row(
            "SELECT started_at, finished_at FROM deployments WHERE id = ?1",
            [&deployment_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(timestamps.0.as_deref(), Some(started_at.as_str()));
    assert_eq!(timestamps.1, None);

    let error = fail_deployment(
        &mut connection,
        &deployment_id,
        "unhealthy",
        "status was 503",
    )
    .unwrap();
    assert_eq!(error.stage, DeploymentStatus::VerifyingInternal);
}

#[test]
fn rejects_skipped_and_repeated_transitions_without_changing_state() {
    let (connection, deployment_id, _) = pending_deployment();

    let skipped = advance_deployment(
        &connection,
        &deployment_id,
        DeploymentTransition::SourcePrepared,
    )
    .unwrap_err();
    assert!(matches!(
        skipped,
        TransitionDeploymentError::Conflict {
            expected: DeploymentStatus::PreparingSource,
            actual: DeploymentStatus::Pending,
            ..
        }
    ));

    advance_deployment(&connection, &deployment_id, DeploymentTransition::Start).unwrap();
    let repeated =
        advance_deployment(&connection, &deployment_id, DeploymentTransition::Start).unwrap_err();
    assert!(matches!(
        repeated,
        TransitionDeploymentError::Conflict {
            expected: DeploymentStatus::Pending,
            actual: DeploymentStatus::PreparingSource,
            ..
        }
    ));
}

#[test]
fn records_a_structured_failure_and_allows_a_later_attempt() {
    let (mut connection, deployment_id, application_id) = pending_deployment();
    advance_deployment(&connection, &deployment_id, DeploymentTransition::Start).unwrap();
    advance_deployment(
        &connection,
        &deployment_id,
        DeploymentTransition::SourcePrepared,
    )
    .unwrap();

    let failure = fail_deployment(
        &mut connection,
        &deployment_id,
        "build_failed",
        "Containerfile failed",
    )
    .unwrap();

    assert_eq!(failure.code, "build_failed");
    assert_eq!(failure.stage, DeploymentStatus::Building);
    assert_eq!(failure.message, "Containerfile failed");
    assert!(!failure.finished_at.is_empty());
    let persisted = connection
        .query_row(
            "SELECT status, failure_code, failure_stage, failure_message, finished_at
             FROM deployments WHERE id = ?1",
            [&deployment_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        persisted,
        (
            "failed".to_owned(),
            "build_failed".to_owned(),
            "building".to_owned(),
            "Containerfile failed".to_owned(),
            failure.finished_at,
        )
    );

    create_deployment(
        &mut connection,
        &application_id,
        &"b".repeat(40),
        Some("main"),
    )
    .unwrap();
}

#[test]
fn terminal_and_missing_deployments_cannot_enter_the_flow() {
    let (mut connection, deployment_id, _) = pending_deployment();
    fail_deployment(
        &mut connection,
        &deployment_id,
        "rejected",
        "operator rejected it",
    )
    .unwrap();

    let terminal =
        advance_deployment(&connection, &deployment_id, DeploymentTransition::Start).unwrap_err();
    let repeated_failure = fail_deployment(
        &mut connection,
        &deployment_id,
        "replacement",
        "must not replace the original failure",
    )
    .unwrap_err();
    let missing =
        advance_deployment(&connection, "missing", DeploymentTransition::Start).unwrap_err();

    assert!(matches!(
        terminal,
        TransitionDeploymentError::Conflict {
            actual: DeploymentStatus::Failed,
            ..
        }
    ));
    assert!(matches!(
        repeated_failure,
        TransitionDeploymentError::CannotFail {
            actual: DeploymentStatus::Failed,
            ..
        }
    ));
    assert!(matches!(
        missing,
        TransitionDeploymentError::DeploymentNotFound { deployment_id }
            if deployment_id == "missing"
    ));
}

#[test]
fn rejects_incomplete_failure_details_without_changing_state() {
    let (mut connection, deployment_id, _) = pending_deployment();

    for (code, message) in [("", "diagnostic"), ("failure", " not trimmed")] {
        let error = fail_deployment(&mut connection, &deployment_id, code, message).unwrap_err();
        assert!(matches!(error, TransitionDeploymentError::InvalidFailure));
    }
    assert_eq!(
        advance_deployment(&connection, &deployment_id, DeploymentTransition::Start).unwrap(),
        DeploymentStatus::PreparingSource
    );
}

fn pending_deployment() -> (rusqlite::Connection, String, String) {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();
    let (_, deployment) = create_deployment(
        &mut connection,
        &application.id,
        &"a".repeat(40),
        Some("main"),
    )
    .unwrap();
    (connection, deployment.id, application.id)
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
