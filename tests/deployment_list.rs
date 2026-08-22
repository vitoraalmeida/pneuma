use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::domain::deployment::{DeploymentStatus, DeploymentType};
use pneuma::domain::release::OciArtifact;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment::create_deployment;
use pneuma::use_cases::deployment::list_deployments;
use pneuma::use_cases::release_create::create_release;

#[test]
fn returns_an_empty_list_for_an_application_without_deployments() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();

    let deployments = list_deployments(&connection, &application.id).unwrap();

    assert!(deployments.is_empty());
}

#[test]
fn returns_deployments_ordered_newest_first() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    let first_release = create_release(&mut connection, &application.id, &artifact('a')).unwrap();
    let second_release = create_release(&mut connection, &application.id, &artifact('b')).unwrap();
    let first_deployment = create_deployment(
        &mut connection,
        &application.id,
        &first_release.id,
        DeploymentType::Deploy,
    )
    .unwrap();
    connection
        .execute(
            "UPDATE deployments
             SET status = 'failed',
                 requested_at = '2026-08-07 10:00:00',
                 finished_at = '2026-08-07 10:01:00',
                 failure_code = 'test_failure',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            [first_deployment.id.as_str()],
        )
        .unwrap();
    let second_deployment = create_deployment(
        &mut connection,
        &application.id,
        &second_release.id,
        DeploymentType::Deploy,
    )
    .unwrap();
    connection
        .execute(
            "UPDATE deployments
             SET status = 'succeeded',
                 requested_at = '2026-08-07 11:00:00',
                 finished_at = '2026-08-07 11:01:00',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            [second_deployment.id.as_str()],
        )
        .unwrap();

    let deployments = list_deployments(&connection, &application.id).unwrap();

    assert_eq!(deployments.len(), 2);
    assert_eq!(deployments[0].deployment.id, second_deployment.id);
    assert_eq!(deployments[0].release.artifact.digest(), digest('b'));
    assert_eq!(
        deployments[0].deployment.status(),
        DeploymentStatus::Succeeded
    );
    assert!(matches!(
        deployments[0].deployment.lifecycle,
        pneuma::domain::deployment::DeploymentLifecycle::Succeeded { .. }
    ));
    assert_eq!(deployments[1].deployment.id, first_deployment.id);
    assert_eq!(deployments[1].release.artifact.digest(), digest('a'));
    assert_eq!(deployments[1].deployment.status(), DeploymentStatus::Failed);
    assert!(matches!(
        deployments[1].deployment.lifecycle,
        pneuma::domain::deployment::DeploymentLifecycle::Failed {
            evidence: pneuma::domain::deployment::DeploymentFailureEvidence::Incomplete
        }
    ));
}

#[test]
fn returns_only_deployments_for_the_given_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let first =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    let second =
        import_application(&mut connection, &fixture_path("another"), None, None, None).unwrap();
    let first_release = create_release(&mut connection, &first.id, &artifact('a')).unwrap();
    let second_release = create_release(&mut connection, &second.id, &artifact('b')).unwrap();
    create_deployment(
        &mut connection,
        &first.id,
        &first_release.id,
        DeploymentType::Deploy,
    )
    .unwrap();
    create_deployment(
        &mut connection,
        &second.id,
        &second_release.id,
        DeploymentType::Deploy,
    )
    .unwrap();

    let first_deployments = list_deployments(&connection, &first.id).unwrap();
    let second_deployments = list_deployments(&connection, &second.id).unwrap();

    assert_eq!(first_deployments.len(), 1);
    assert_eq!(first_deployments[0].release.artifact.digest(), digest('a'));
    assert_eq!(second_deployments.len(), 1);
    assert_eq!(second_deployments[0].release.artifact.digest(), digest('b'));
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn artifact(character: char) -> OciArtifact {
    OciArtifact::new("localhost/test", &digest(character)).unwrap()
}

fn digest(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}
