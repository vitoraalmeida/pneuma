use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::domain::deployment::{DeploymentStatus, DeploymentType};
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_create::{
    CreateDeploymentError, create_deployment, create_deployment_with_source_revision,
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
    )
    .unwrap();
    let second_release = create_release(
        &mut connection,
        &application.id,
        &format!("localhost/test:{}", "b".repeat(40)),
        "localhost/test",
        &"b".repeat(40),
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
fn preserves_provenance_for_each_attempt_using_the_same_release() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    let release = create_release(
        &mut connection,
        &application.id,
        &format!("localhost/test:{}", "a".repeat(40)),
        "localhost/test",
        &"a".repeat(40),
    )
    .unwrap();

    let first = create_deployment_with_source_revision(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
        Some("first-commit"),
    )
    .unwrap();
    connection
        .execute(
            "UPDATE deployments SET status = 'failed' WHERE id = ?1",
            [&first.id],
        )
        .unwrap();
    let second = create_deployment_with_source_revision(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
        None,
    )
    .unwrap();

    assert_eq!(first.source_revision.as_deref(), Some("first-commit"));
    assert_eq!(second.source_revision, None);
    let release_source_revision: Option<String> = connection
        .query_row("SELECT source_revision FROM releases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(release_source_revision, None);
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
fn rejects_a_missing_release_for_an_existing_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();

    let error = create_deployment(
        &mut connection,
        &application.id,
        "missing-release",
        DeploymentType::Deploy,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CreateDeploymentError::ReleaseNotFound { release_id }
            if release_id == "missing-release"
    ));
}

#[test]
fn a_running_active_runtime_blocks_deploying_the_same_release() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, release_id) = setup_active_runtime(&mut connection, "running", None);

    let error = create_deployment(
        &mut connection,
        &application_id,
        &release_id,
        DeploymentType::Deploy,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CreateDeploymentError::AlreadyActive { release_id: actual } if actual == release_id
    ));
}

#[test]
fn a_stopped_active_runtime_blocks_deploying_the_same_release() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, release_id) = setup_active_runtime(&mut connection, "stopped", None);

    let error = create_deployment(
        &mut connection,
        &application_id,
        &release_id,
        DeploymentType::Deploy,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CreateDeploymentError::AlreadyActive { release_id: actual } if actual == release_id
    ));
}

#[test]
fn a_removed_active_runtime_does_not_block_deployment() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, release_id) =
        setup_active_runtime(&mut connection, "running", Some("2000-01-01 00:00:00"));

    let deployment = create_deployment(
        &mut connection,
        &application_id,
        &release_id,
        DeploymentType::Deploy,
    )
    .unwrap();

    assert_eq!(deployment.status, DeploymentStatus::Pending);
}

#[test]
fn rollback_of_the_active_release_is_allowed() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, release_id) = setup_active_runtime(&mut connection, "running", None);

    let deployment = create_deployment(
        &mut connection,
        &application_id,
        &release_id,
        DeploymentType::Rollback,
    )
    .unwrap();

    assert_eq!(deployment.deployment_type, DeploymentType::Rollback);
    assert_eq!(deployment.status, DeploymentStatus::Pending);
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

fn setup_active_runtime(
    connection: &mut rusqlite::Connection,
    runtime_state: &str,
    removed_at: Option<&str>,
) -> (String, String) {
    let application =
        import_application(connection, &fixture_path("valid"), None, None, None).unwrap();
    let release = create_release(
        connection,
        &application.id,
        &format!("localhost/test:{}", "a".repeat(40)),
        "localhost/test",
        &"a".repeat(40),
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO deployments (
                id, application_id, release_id, type, status, created_at, updated_at
             ) VALUES ('active-deployment', ?1, ?2, 'deploy', 'succeeded',
                       CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            [&application.id, &release.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runtime_instances (
                id, application_id, deployment_id, external_runtime_id, state,
                host_address, host_port, container_port, last_observed_state,
                last_observed_at, created_at, updated_at, removed_at
             ) VALUES ('active-runtime', ?1, 'active-deployment', 'aabbccdd', ?2,
                       '127.0.0.1', 30000, 8080, ?2, CURRENT_TIMESTAMP,
                       CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?3)",
            rusqlite::params![application.id, runtime_state, removed_at],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE applications SET active_deployment_id = 'active-deployment' WHERE id = ?1",
            [&application.id],
        )
        .unwrap();

    (application.id, release.id)
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
