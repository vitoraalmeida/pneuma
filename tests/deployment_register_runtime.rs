use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::adapters::local_runtime::ObservedRuntimeState;
use pneuma::domain::deployment::DeploymentType;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_create::create_deployment;
use pneuma::use_cases::deployment_register_runtime::{
    RegisterCandidateRuntimeError, register_candidate_runtime,
};
use pneuma::use_cases::deployment_transition::{DeploymentTransition, advance_deployment};
use pneuma::use_cases::release_create::create_release;

#[test]
fn persists_a_running_candidate_linked_to_its_deployment() {
    let (mut connection, application_id, deployment_id) = starting_deployment("valid");
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
    assert_eq!(runtime.deployment_id, deployment_id);
    assert_eq!(runtime.external_runtime_id, external_runtime_id);
    assert_eq!(runtime.endpoint, endpoint);
    assert_eq!(runtime.container_port, 8080);
    assert_eq!(runtime.observed_state, ObservedRuntimeState::Running);
    assert!(!runtime.observed_at.is_empty());
    let state: String = connection
        .query_row(
            "SELECT state FROM runtime_instances WHERE id = ?1",
            [&runtime.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "starting");
}

#[test]
fn requires_a_starting_deployment() {
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
    let deployment = create_deployment(
        &mut connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
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
    let (mut connection, _, deployment_id) = starting_deployment("valid");

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
        "0.0.0.0:30001".parse().unwrap(),
        8080,
    )
    .unwrap_err();
    let invalid_port = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &"b".repeat(64),
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
}

#[test]
fn identical_retry_is_idempotent_but_conflicting_reuse_is_rejected() {
    let (mut connection, _, deployment_id) = starting_deployment("valid");
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

    let identical = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &external_runtime_id,
        endpoint,
        8080,
    )
    .unwrap();
    let conflicting_endpoint = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &"b".repeat(64),
        endpoint,
        8080,
    )
    .unwrap_err();
    let conflicting_external_id = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &external_runtime_id,
        "127.0.0.1:30002".parse().unwrap(),
        8080,
    )
    .unwrap_err();

    assert_eq!(identical, runtime);
    assert!(matches!(
        conflicting_endpoint,
        RegisterCandidateRuntimeError::EndpointConflict { endpoint }
            if endpoint == endpoint
    ));
    assert!(matches!(
        conflicting_external_id,
        RegisterCandidateRuntimeError::ExternalRuntimeConflict { .. }
    ));
}

#[test]
fn database_rejects_a_duplicate_active_endpoint() {
    let (mut connection, _, deployment_id) = starting_deployment("valid");
    let endpoint: SocketAddr = "127.0.0.1:30001".parse().unwrap();
    let runtime = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &"a".repeat(64),
        endpoint,
        8080,
    )
    .unwrap();

    let error = connection
        .execute(
            "INSERT INTO runtime_instances (
                id, application_id, deployment_id, external_runtime_id,
                state, host_address, host_port, container_port,
                last_observed_state, last_observed_at
             ) VALUES (
                'other', ?1, ?2, ?3,
                'starting', '127.0.0.1', ?4, 8080,
                'running', CURRENT_TIMESTAMP
             )",
            rusqlite::params![
                runtime.application_id,
                runtime.deployment_id,
                "b".repeat(64),
                endpoint.port()
            ],
        )
        .unwrap_err();

    assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
}

#[test]
fn database_rejects_a_runtime_identity_from_another_application() {
    let (mut connection, _, deployment_id) = starting_deployment("valid");
    let runtime = register_candidate_runtime(
        &mut connection,
        &deployment_id,
        &"a".repeat(64),
        "127.0.0.1:30001".parse().unwrap(),
        8080,
    )
    .unwrap();
    let second =
        import_application(&mut connection, &fixture_path("another"), None, None, None).unwrap();

    let error = connection
        .execute(
            "UPDATE runtime_instances SET application_id = ?1 WHERE id = ?2",
            [&second.id, &runtime.id],
        )
        .unwrap_err();

    assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
}

fn starting_deployment(fixture: &str) -> (rusqlite::Connection, String, String) {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (application_id, deployment_id) = add_starting_deployment(&mut connection, fixture);
    (connection, application_id, deployment_id)
}

fn add_starting_deployment(
    connection: &mut rusqlite::Connection,
    fixture: &str,
) -> (String, String) {
    let application =
        import_application(connection, &fixture_path(fixture), None, None, None).unwrap();
    let commit_sha = if fixture == "valid" {
        "a".repeat(40)
    } else {
        "b".repeat(40)
    };
    let release = create_release(
        connection,
        &application.id,
        &format!("localhost/test:{commit_sha}"),
        "localhost/test",
        &commit_sha,
        None,
    )
    .unwrap();
    let deployment = create_deployment(
        connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
    )
    .unwrap();
    advance_deployment(connection, &deployment.id, DeploymentTransition::Start).unwrap();
    (application.id, deployment.id)
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
