use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::adapters::local_runtime::ObservedRuntimeState;
use pneuma::use_cases::create_deployment::create_deployment;
use pneuma::use_cases::import_application::import_application;
use pneuma::use_cases::register_candidate_runtime::{
    RegisterCandidateRuntimeError, register_candidate_runtime,
};
use pneuma::use_cases::transition_deployment::{DeploymentTransition, advance_deployment};

#[test]
fn persists_a_running_candidate_linked_to_its_deployment() {
    let (mut connection, application_id, revision_id, deployment_id) = starting_deployment("valid");
    let external_runtime_id = "a".repeat(64);
    let endpoint: SocketAddr = "127.0.0.1:30001".parse().unwrap();

    let runtime = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &external_runtime_id,
        endpoint,
        8080,
    )
    .unwrap();

    assert_eq!(runtime.application_id, application_id);
    assert_eq!(runtime.revision_id, revision_id);
    assert_eq!(runtime.deployment_id, deployment_id);
    assert_eq!(runtime.external_runtime_id, external_runtime_id);
    assert_eq!(runtime.endpoint, endpoint);
    assert_eq!(runtime.container_port, 8080);
    assert_eq!(runtime.observed_state, ObservedRuntimeState::Running);
    assert!(!runtime.observed_at.is_empty());
    let role: String = connection
        .query_row(
            "SELECT role FROM runtime_instances WHERE id = ?1",
            [&runtime.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(role, "candidate");
}

#[test]
fn requires_a_starting_deployment() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();
    let (_, deployment) = create_deployment(
        &mut connection,
        &application.id,
        &"a".repeat(40),
        Some("main"),
    )
    .unwrap();

    let error = register_candidate_runtime(
        &mut connection,
        &deployment.id,
        &"b".repeat(64),
        "127.0.0.1:30001".parse().unwrap(),
        8080,
    )
    .unwrap_err();
    let missing = register_candidate_runtime(
        &mut connection,
        "missing",
        &"c".repeat(64),
        "127.0.0.1:30002".parse().unwrap(),
        8080,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        RegisterCandidateRuntimeError::InvalidDeploymentState { actual, .. }
            if actual == "pending"
    ));
    assert!(matches!(
        missing,
        RegisterCandidateRuntimeError::DeploymentNotFound { deployment_id }
            if deployment_id == "missing"
    ));
}

#[test]
fn rejects_invalid_runtime_coordinates_before_writing() {
    let (mut connection, _, _, deployment_id) = starting_deployment("valid");

    let invalid_id = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        "not-hex",
        "127.0.0.1:30001".parse().unwrap(),
        8080,
    )
    .unwrap_err();
    let invalid_endpoint = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &"a".repeat(64),
        "192.0.2.1:30001".parse().unwrap(),
        8080,
    )
    .unwrap_err();
    let invalid_port = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &"a".repeat(64),
        "127.0.0.1:30001".parse().unwrap(),
        0,
    )
    .unwrap_err();

    assert!(matches!(
        invalid_id,
        RegisterCandidateRuntimeError::InvalidExternalRuntimeId
    ));
    assert!(matches!(
        invalid_endpoint,
        RegisterCandidateRuntimeError::InvalidEndpoint { .. }
    ));
    assert!(matches!(
        invalid_port,
        RegisterCandidateRuntimeError::InvalidContainerPort
    ));
    let runtime_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM runtime_instances", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(runtime_count, 0);
}

#[test]
fn identical_retry_is_idempotent_but_conflicting_reuse_is_rejected() {
    let (mut connection, _, _, deployment_id) = starting_deployment("valid");
    let external_runtime_id = "a".repeat(64);
    let endpoint: SocketAddr = "127.0.0.1:30001".parse().unwrap();
    let first = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &external_runtime_id,
        endpoint,
        8080,
    )
    .unwrap();

    let repeated = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &external_runtime_id,
        endpoint,
        8080,
    )
    .unwrap();
    let conflict = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &external_runtime_id,
        "127.0.0.1:30002".parse().unwrap(),
        8080,
    )
    .unwrap_err();

    assert_eq!(repeated, first);
    assert!(matches!(
        conflict,
        RegisterCandidateRuntimeError::ExternalRuntimeConflict { .. }
    ));
}

#[test]
fn database_rejects_a_duplicate_active_endpoint() {
    let (mut connection, _, _, first_deployment_id) = starting_deployment("valid");
    let (_, _, second_deployment_id) = add_starting_deployment(&mut connection, "another");
    let endpoint: SocketAddr = "127.0.0.1:30001".parse().unwrap();
    register_candidate_runtime(
        &mut connection,
        &first_deployment_id,
        &"a".repeat(64),
        endpoint,
        8080,
    )
    .unwrap();

    let error = register_candidate_runtime(
        &mut connection,
        &second_deployment_id,
        &"b".repeat(64),
        endpoint,
        8081,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        RegisterCandidateRuntimeError::EndpointConflict { endpoint: actual }
            if actual == endpoint
    ));
}

#[test]
fn database_rejects_a_runtime_identity_from_another_application() {
    let (mut connection, _, _, deployment_id) = starting_deployment("valid");
    let runtime = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &"a".repeat(64),
        "127.0.0.1:30001".parse().unwrap(),
        8080,
    )
    .unwrap();
    let second = import_application(&mut connection, &fixture_path("another")).unwrap();

    let error = connection
        .execute(
            "UPDATE runtime_instances SET application_id = ?1 WHERE id = ?2",
            [&second.id, &runtime.id],
        )
        .unwrap_err();

    assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
}

fn starting_deployment(fixture: &str) -> (rusqlite::Connection, String, String, String) {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, revision_id, deployment_id) =
        add_starting_deployment(&mut connection, fixture);
    (connection, application_id, revision_id, deployment_id)
}

fn add_starting_deployment(
    connection: &mut rusqlite::Connection,
    fixture: &str,
) -> (String, String, String) {
    let application = import_application(connection, &fixture_path(fixture)).unwrap();
    let commit_sha = if fixture == "valid" {
        "a".repeat(40)
    } else {
        "b".repeat(40)
    };
    let (revision, deployment) =
        create_deployment(connection, &application.id, &commit_sha, Some("main")).unwrap();
    advance_deployment(connection, &deployment.id, DeploymentTransition::Start).unwrap();
    advance_deployment(
        connection,
        &deployment.id,
        DeploymentTransition::SourcePrepared,
    )
    .unwrap();
    advance_deployment(connection, &deployment.id, DeploymentTransition::ImageBuilt).unwrap();
    (application.id, revision.id, deployment.id)
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
