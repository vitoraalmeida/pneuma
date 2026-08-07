use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_create::{DeploymentStatus, create_deployment};
use pneuma::use_cases::deployment_list::list_deployments;

#[test]
fn returns_an_empty_list_for_an_application_without_deployments() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();

    let deployments = list_deployments(&connection, &application.id).unwrap();

    assert!(deployments.is_empty());
}

#[test]
fn returns_deployments_ordered_newest_first() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();
    let first_commit = "a".repeat(40);
    let second_commit = "b".repeat(40);
    let (_, first_deployment) =
        create_deployment(&mut connection, &application.id, &first_commit, None).unwrap();
    connection
        .execute(
            "UPDATE deployments
             SET status = 'failed',
                 requested_at = '2026-08-07 10:00:00',
                 finished_at = '2026-08-07 10:01:00',
                 failure_code = 'test_failure',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            [&first_deployment.id],
        )
        .unwrap();
    let (_, second_deployment) =
        create_deployment(&mut connection, &application.id, &second_commit, None).unwrap();
    connection
        .execute(
            "UPDATE deployments
             SET status = 'succeeded',
                 requested_at = '2026-08-07 11:00:00',
                 finished_at = '2026-08-07 11:01:00',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            [&second_deployment.id],
        )
        .unwrap();

    let deployments = list_deployments(&connection, &application.id).unwrap();

    assert_eq!(deployments.len(), 2);
    assert_eq!(deployments[0].id, second_deployment.id);
    assert_eq!(deployments[0].commit_sha, second_commit);
    assert_eq!(deployments[0].status, DeploymentStatus::Succeeded);
    assert!(deployments[0].finished_at.is_some());
    assert_eq!(deployments[1].id, first_deployment.id);
    assert_eq!(deployments[1].commit_sha, first_commit);
    assert_eq!(deployments[1].status, DeploymentStatus::Failed);
    assert!(deployments[1].finished_at.is_some());
}

#[test]
fn returns_only_deployments_for_the_given_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let first = import_application(&mut connection, &fixture_path("valid")).unwrap();
    let second = import_application(&mut connection, &fixture_path("another")).unwrap();
    create_deployment(&mut connection, &first.id, &"a".repeat(40), None).unwrap();
    create_deployment(&mut connection, &second.id, &"b".repeat(40), None).unwrap();

    let first_deployments = list_deployments(&connection, &first.id).unwrap();
    let second_deployments = list_deployments(&connection, &second.id).unwrap();

    assert_eq!(first_deployments.len(), 1);
    assert_eq!(first_deployments[0].commit_sha, "a".repeat(40));
    assert_eq!(second_deployments.len(), 1);
    assert_eq!(second_deployments[0].commit_sha, "b".repeat(40));
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
