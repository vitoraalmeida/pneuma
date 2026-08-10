use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_create::{
    CreateDeploymentError, DeploymentStatus, DeploymentType, create_deployment,
};
use pneuma::use_cases::release_create::create_release;

#[test]
fn persists_a_pending_deployment_atomically() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    let release = create_release(
        &mut connection,
        &application.id,
        &format!("localhost/test:{}", "a".repeat(40)),
        "localhost/test",
        &"a".repeat(40),
        Some(&"a".repeat(40)),
    )
    .unwrap();

    let deployment = create_deployment(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
    )
    .unwrap();

    assert_eq!(deployment.application_id, application.id);
    assert_eq!(deployment.release_id, release.id);
    assert_eq!(deployment.status, DeploymentStatus::Pending);
    assert!(!deployment.requested_at.is_empty());
}

#[test]
fn rejects_a_second_active_deployment_for_the_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    let first_release = create_release(
        &mut connection,
        &application.id,
        &format!("localhost/test:{}", "a".repeat(40)),
        "localhost/test",
        &"a".repeat(40),
        None,
    )
    .unwrap();
    let second_release = create_release(
        &mut connection,
        &application.id,
        &format!("localhost/test:{}", "b".repeat(40)),
        "localhost/test",
        &"b".repeat(40),
        None,
    )
    .unwrap();
    create_deployment(
        &mut connection,
        &application.id,
        &first_release.id,
        DeploymentType::Deploy,
    )
    .unwrap();

    let error = create_deployment(
        &mut connection,
        &application.id,
        &second_release.id,
        DeploymentType::Deploy,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CreateDeploymentError::ActiveDeployment { application_id }
            if application_id == application.id
    ));
    let release_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM releases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(release_count, 2);
}

#[test]
fn reuses_a_release_for_a_later_deployment_attempt() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    let release = create_release(
        &mut connection,
        &application.id,
        &format!("localhost/test:{}", "a".repeat(40)),
        "localhost/test",
        &"a".repeat(40),
        None,
    )
    .unwrap();
    let first_deployment = create_deployment(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
    )
    .unwrap();
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

    let second_deployment = create_deployment(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
    )
    .unwrap();

    assert_eq!(second_deployment.release_id, first_deployment.release_id);
    assert_ne!(second_deployment.id, first_deployment.id);
    let release_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM releases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(release_count, 1);
}

#[test]
fn rejects_a_missing_release_and_missing_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let missing_release = create_deployment(
        &mut connection,
        "missing",
        "missing-release",
        DeploymentType::Deploy,
    )
    .unwrap_err();

    assert!(matches!(
        missing_release,
        CreateDeploymentError::ApplicationNotFound { application_id }
            if application_id == "missing"
    ));
}

#[test]
fn database_rejects_a_release_from_another_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let first =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    let second =
        import_application(&mut connection, &fixture_path("another"), None, None, None).unwrap();
    let release = create_release(
        &mut connection,
        &first.id,
        &format!("localhost/test:{}", "a".repeat(40)),
        "localhost/test",
        &"a".repeat(40),
        None,
    )
    .unwrap();
    let deployment = create_deployment(
        &mut connection,
        &first.id,
        &release.id,
        DeploymentType::Deploy,
    )
    .unwrap();

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
