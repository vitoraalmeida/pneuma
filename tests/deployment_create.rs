use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::database;
use pneuma::domain::deployment::{DeploymentStatus, DeploymentType};
use pneuma::domain::git::CommitSha;
use pneuma::domain::identity::{ApplicationId, ReleaseId};
use pneuma::domain::release::OciArtifact;
use pneuma::use_cases::application::import_application;
use pneuma::use_cases::deployment::{
    CreateDeploymentError, create_deployment, create_deployment_with_source_revision,
};
use pneuma::use_cases::release::create_release;
use rusqlite::{ErrorCode, TransactionBehavior};

#[test]
fn immediate_transaction_acquires_the_writer_lock_before_reading() {
    let database_path = temporary_database_path();
    let mut first = database::open(&database_path).unwrap();
    let mut second = database::open(&database_path).unwrap();
    second.busy_timeout(std::time::Duration::ZERO).unwrap();

    let transaction = first
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let error = create_deployment(
        &mut second,
        &ApplicationId::new("33333333333333333333333333333333").unwrap(),
        &ReleaseId::new("55555555555555555555555555555555").unwrap(),
        DeploymentType::Deploy,
    )
    .unwrap_err();
    drop(transaction);
    let _ = std::fs::remove_file(&database_path);

    let CreateDeploymentError::Persistence { source } = &error else {
        panic!("expected the writer-lock conflict to surface as a persistence error: {error:?}");
    };
    let rusqlite::Error::SqliteFailure(failure, _) = source
        .downcast_ref::<rusqlite::Error>()
        .expect("SQLite failures must stay downcastable")
    else {
        panic!("expected a SQLite failure, got {source:?}");
    };
    assert_eq!(failure.code, ErrorCode::DatabaseBusy);
}

fn temporary_database_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "pneuma-deployment-create-{}-{}.sqlite3",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn persists_a_pending_deployment_atomically() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let release = create_release(&mut connection, &application.id, &artifact('a')).unwrap();

    let deployment = create_deployment(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
    )
    .unwrap();

    assert_eq!(deployment.application_id, application.id);
    assert_eq!(deployment.release_id, release.id);
    assert_eq!(deployment.status(), DeploymentStatus::Pending);
    assert!(!deployment.requested_at.is_empty());
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn artifact(character: char) -> OciArtifact {
    OciArtifact::new(
        "localhost/test",
        &format!("sha256:{}", character.to_string().repeat(64)),
    )
    .unwrap()
}

#[test]
fn rejects_a_second_active_deployment_for_the_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let first_release = create_release(&mut connection, &application.id, &artifact('a')).unwrap();
    let second_release = create_release(&mut connection, &application.id, &artifact('b')).unwrap();
    let first_deployment = create_deployment(
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
        CreateDeploymentError::ActiveDeployment { deployment }
            if deployment.id == first_deployment.id
    ));
    let release_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM releases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(release_count, 2);
}

#[test]
fn reuses_a_release_for_a_later_deployment_attempt() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let release = create_release(&mut connection, &application.id, &artifact('a')).unwrap();
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
                 failure_code = 'test_gate_failed',
                 failure_stage = 'starting',
                 failure_message = 'test'
             WHERE id = ?1",
            [first_deployment.id.as_str()],
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
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let release = create_release(&mut connection, &application.id, &artifact('a')).unwrap();

    let first = create_deployment_with_source_revision(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
        Some(&CommitSha::new(&"a".repeat(40)).unwrap()),
    )
    .unwrap();
    connection
        .execute(
            "UPDATE deployments
             SET status = 'failed', finished_at = CURRENT_TIMESTAMP,
                 failure_code = 'test_gate_failed', failure_stage = 'starting',
                 failure_message = 'test'
             WHERE id = ?1",
            [first.id.as_str()],
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

    assert_eq!(
        first
            .source_revision
            .as_ref()
            .map(|commit| commit.as_str().to_owned()),
        Some("a".repeat(40))
    );
    assert_eq!(second.source_revision, None);
    let release_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('releases')
             WHERE name = 'source_revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        release_columns, 0,
        "releases must not carry source provenance"
    );
}

#[test]
fn rejects_a_missing_release_and_missing_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let missing_release = create_deployment(
        &mut connection,
        &ApplicationId::new("33333333333333333333333333333333").unwrap(),
        &ReleaseId::new("55555555555555555555555555555555").unwrap(),
        DeploymentType::Deploy,
    )
    .unwrap_err();

    assert!(matches!(
        missing_release,
        CreateDeploymentError::ApplicationNotFound { application_id }
            if application_id == "33333333333333333333333333333333"
    ));
}

#[test]
fn rejects_a_missing_release_for_an_existing_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();

    let error = create_deployment(
        &mut connection,
        &application.id,
        &ReleaseId::new("55555555555555555555555555555555").unwrap(),
        DeploymentType::Deploy,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CreateDeploymentError::ReleaseNotFound { release_id }
            if release_id == "55555555555555555555555555555555"
    ));
}

#[test]
fn a_running_active_runtime_blocks_deploying_the_same_release() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, release_id) = setup_active_runtime(&mut connection, "running", None);

    let error = create_deployment(
        &mut connection,
        &ApplicationId::new(&application_id).unwrap(),
        &ReleaseId::new(&release_id.clone()).unwrap(),
        DeploymentType::Deploy,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CreateDeploymentError::AlreadyActive { release_id: actual } if actual == release_id
    ));
}

fn setup_active_runtime(
    connection: &mut rusqlite::Connection,
    runtime_state: &str,
    removed_at: Option<&str>,
) -> (String, String) {
    let application = import_application(
        connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let release = create_release(connection, &application.id, &artifact('a')).unwrap();
    connection
        .execute(
            "INSERT INTO deployments (
                id, application_id, release_id, type, status, requested_at, finished_at
             ) VALUES ('cccccccccccccccccccccccccccccccc', ?1, ?2, 'deploy', 'succeeded',
                       CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            rusqlite::params![application.id.as_str(), release.id.as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runtime_instances (
                id, application_id, deployment_id, external_runtime_id, state,
                host_port, container_port, last_observed_state,
                last_observed_at, removed_at
             ) VALUES ('dddddddddddddddddddddddddddddddd', ?1, 'cccccccccccccccccccccccccccccccc', 'aabbccdd', ?2,
                       30000, 8080, ?2, CURRENT_TIMESTAMP, ?3)",
            rusqlite::params![application.id.as_str(), runtime_state, removed_at],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE applications SET active_deployment_id = 'cccccccccccccccccccccccccccccccc' WHERE id = ?1",
            [application.id.as_str()],
        )
        .unwrap();

    (application.id.to_string(), release.id.to_string())
}

#[test]
fn a_stopped_active_runtime_blocks_deploying_the_same_release() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, release_id) = setup_active_runtime(&mut connection, "stopped", None);

    let error = create_deployment(
        &mut connection,
        &ApplicationId::new(&application_id).unwrap(),
        &ReleaseId::new(&release_id.clone()).unwrap(),
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
        setup_active_runtime(&mut connection, "stopped", Some("2000-01-01 00:00:00"));

    let deployment = create_deployment(
        &mut connection,
        &ApplicationId::new(&application_id).unwrap(),
        &ReleaseId::new(&release_id).unwrap(),
        DeploymentType::Deploy,
    )
    .unwrap();

    assert_eq!(deployment.status(), DeploymentStatus::Pending);
}

#[test]
fn rollback_of_the_active_release_is_allowed() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, release_id) = setup_active_runtime(&mut connection, "running", None);

    let deployment = create_deployment(
        &mut connection,
        &ApplicationId::new(&application_id).unwrap(),
        &ReleaseId::new(&release_id).unwrap(),
        DeploymentType::Rollback,
    )
    .unwrap();

    assert_eq!(deployment.deployment_type, DeploymentType::Rollback);
    assert_eq!(deployment.status(), DeploymentStatus::Pending);
}

#[test]
fn database_rejects_a_release_from_another_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let first = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let second = import_application(
        &mut connection,
        &fixture_path("another"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let release = create_release(&mut connection, &first.id, &artifact('a')).unwrap();
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
            rusqlite::params![second.id.as_str(), deployment.id.as_str()],
        )
        .unwrap_err();

    assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
}
