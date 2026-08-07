use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_create::{
    CreateDeploymentError, DeploymentStatus, create_deployment,
};

#[test]
fn persists_a_revision_and_pending_deployment_atomically() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();
    let commit_sha = "a".repeat(40);

    let (revision, deployment) =
        create_deployment(&mut connection, &application.id, &commit_sha, Some("main")).unwrap();

    assert_eq!(revision.application_id, application.id);
    assert_eq!(revision.commit_sha, commit_sha);
    assert_eq!(revision.source_reference.as_deref(), Some("main"));
    assert!(!revision.discovered_at.is_empty());
    assert_eq!(deployment.application_id, application.id);
    assert_eq!(deployment.revision_id, revision.id);
    assert_eq!(deployment.status, DeploymentStatus::Pending);
    assert!(!deployment.requested_at.is_empty());
}

#[test]
fn rejects_a_second_active_deployment_for_the_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();
    let first_commit = "a".repeat(40);
    let second_commit = "b".repeat(40);
    create_deployment(
        &mut connection,
        &application.id,
        &first_commit,
        Some("main"),
    )
    .unwrap();

    let error = create_deployment(
        &mut connection,
        &application.id,
        &second_commit,
        Some("main"),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CreateDeploymentError::ActiveDeployment { application_id }
            if application_id == application.id
    ));
    let revision_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM revisions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(revision_count, 1);
}

#[test]
fn reuses_a_revision_for_a_later_deployment_attempt() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();
    let commit_sha = "a".repeat(40);
    let (first_revision, first_deployment) =
        create_deployment(&mut connection, &application.id, &commit_sha, Some("main")).unwrap();
    connection
        .execute(
            "UPDATE deployments
             SET status = 'failed',
                 finished_at = CURRENT_TIMESTAMP,
                 failure_code = 'test_failure',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            [&first_deployment.id],
        )
        .unwrap();

    let (second_revision, second_deployment) =
        create_deployment(&mut connection, &application.id, &commit_sha, Some("main")).unwrap();

    assert_eq!(second_revision.id, first_revision.id);
    assert_ne!(second_deployment.id, first_deployment.id);
    let revision_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM revisions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(revision_count, 1);
}

#[test]
fn rejects_an_invalid_commit_and_missing_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let invalid_commit = create_deployment(&mut connection, "missing", "abc", None).unwrap_err();
    let missing_application =
        create_deployment(&mut connection, "missing", &"a".repeat(40), None).unwrap_err();

    assert!(matches!(
        invalid_commit,
        CreateDeploymentError::InvalidCommit
    ));
    assert!(matches!(
        missing_application,
        CreateDeploymentError::ApplicationNotFound { application_id }
            if application_id == "missing"
    ));
}

#[test]
fn database_rejects_a_revision_from_another_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let first = import_application(&mut connection, &fixture_path("valid")).unwrap();
    let second = import_application(&mut connection, &fixture_path("another")).unwrap();
    let (_, deployment) =
        create_deployment(&mut connection, &first.id, &"a".repeat(40), None).unwrap();

    let error = connection
        .execute(
            "UPDATE deployments SET application_id = ?1 WHERE id = ?2",
            [&second.id, &deployment.id],
        )
        .unwrap_err();

    assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
