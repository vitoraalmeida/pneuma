use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_create::{DeploymentStatus, DeploymentType, create_deployment};
use pneuma::use_cases::deployment_transition::{
    DeploymentTransition, TransitionDeploymentError, advance_deployment, fail_deployment,
};
use pneuma::use_cases::release_create::create_release;

#[test]
fn advances_in_order_through_internal_verification() {
    let (mut connection, deployment_id, _) = pending_deployment();

    assert_eq!(
        advance_deployment(&connection, &deployment_id, DeploymentTransition::Start).unwrap(),
        DeploymentStatus::Starting
    );
    let started_at: String = connection
        .query_row(
            "SELECT started_at FROM deployments WHERE id = ?1",
            [&deployment_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        advance_deployment(
            &connection,
            &deployment_id,
            DeploymentTransition::RuntimeRunning
        )
        .unwrap(),
        DeploymentStatus::Verifying
    );

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
    assert_eq!(error.stage, DeploymentStatus::Verifying);
}

#[test]
fn advances_through_public_verification_and_can_fail_there() {
    let (mut connection, deployment_id, _) = pending_deployment();
    for transition in [
        DeploymentTransition::Start,
        DeploymentTransition::RuntimeRunning,
        DeploymentTransition::Verified,
    ] {
        advance_deployment(&connection, &deployment_id, transition).unwrap();
    }

    let failure = fail_deployment(
        &mut connection,
        &deployment_id,
        "external_health_check_failed",
        "public endpoint returned 503",
    )
    .unwrap();

    assert_eq!(failure.stage, DeploymentStatus::Activating);
}

#[test]
fn rejects_skipped_and_repeated_transitions_without_changing_state() {
    let (connection, deployment_id, _) = pending_deployment();

    let skipped = advance_deployment(
        &connection,
        &deployment_id,
        DeploymentTransition::RuntimeRunning,
    )
    .unwrap_err();
    assert!(matches!(
        skipped,
        TransitionDeploymentError::Conflict {
            expected: DeploymentStatus::Starting,
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
            actual: DeploymentStatus::Starting,
            ..
        }
    ));
}

#[test]
fn records_a_structured_failure_and_allows_a_later_attempt() {
    let (mut connection, deployment_id, application_id) = pending_deployment();
    advance_deployment(&connection, &deployment_id, DeploymentTransition::Start).unwrap();

    let failure = fail_deployment(
        &mut connection,
        &deployment_id,
        "runtime_failed",
        "container exited",
    )
    .unwrap();

    assert_eq!(failure.code, "runtime_failed");
    assert_eq!(failure.stage, DeploymentStatus::Starting);
    assert_eq!(failure.message, "container exited");
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
            "runtime_failed".to_owned(),
            "starting".to_owned(),
            "container exited".to_owned(),
            failure.finished_at,
        )
    );

    let release = create_release(
        &mut connection,
        &application_id,
        &format!("localhost/test:{}", "b".repeat(40)),
        "localhost/test",
        &"b".repeat(40),
        None,
    )
    .unwrap();
    create_deployment(
        &mut connection,
        &application_id,
        &release.id,
        DeploymentType::Deploy,
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
        DeploymentStatus::Starting
    );
}

fn pending_deployment() -> (rusqlite::Connection, String, String) {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid"), None).unwrap();
    let release = create_release(
        &mut connection,
        &application.id,
        &format!("localhost/test:{}", "a".repeat(40)),
        "localhost/test",
        &"a".repeat(40),
        None,
    )
    .unwrap();
    let deployment = create_deployment(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
    )
    .unwrap();
    (connection, deployment.id, application.id)
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
