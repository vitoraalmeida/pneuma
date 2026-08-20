use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

use pneuma::adapters::database;
use pneuma::domain::deployment::DeploymentType;
use pneuma::domain::release::OciArtifact;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_create::create_deployment;
use pneuma::use_cases::deployment_promote_internal::{
    PromoteInternalCandidateError, promote_internal_candidate,
};
use pneuma::use_cases::deployment_register_runtime::register_candidate_runtime;
use pneuma::use_cases::deployment_transition::{DeploymentTransition, advance_deployment};
use pneuma::use_cases::release_create::create_release;

#[test]
fn promotes_a_healthy_internal_candidate_idempotently() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (endpoint, server) = server_with_statuses(&[200]);
    let runtime_id = add_verifying_candidate(&mut connection, "another", 'a', 'b', endpoint);

    let promoted =
        promote_internal_candidate(&mut connection, &runtime_id, "/healthz", 200).unwrap();
    server.join().unwrap();
    let repeated =
        promote_internal_candidate(&mut connection, &runtime_id, "/healthz", 200).unwrap();

    assert_eq!(repeated, promoted);
    let persisted = runtime_and_deployment_state(&connection, &runtime_id);
    assert_eq!(persisted.0, "running");
    assert_eq!(persisted.1, "succeeded");
    assert_eq!(persisted.2.as_deref(), Some(promoted.finished_at.as_str()));
    let desired_state: String = connection
        .query_row(
            "SELECT desired_runtime_state FROM applications
             WHERE id = (SELECT application_id FROM runtime_instances WHERE id = ?1)",
            [&runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(desired_state, "running");
}

#[test]
fn replaces_the_previous_current_runtime_atomically() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (first_endpoint, first_server) = server_with_statuses(&[200]);
    let first_runtime =
        add_verifying_candidate(&mut connection, "another", 'a', 'b', first_endpoint);
    promote_internal_candidate(&mut connection, &first_runtime, "/healthz", 200).unwrap();
    first_server.join().unwrap();

    let (second_endpoint, second_server) = server_with_statuses(&[200]);
    let second_runtime =
        add_verifying_candidate(&mut connection, "another", 'c', 'd', second_endpoint);
    promote_internal_candidate(&mut connection, &second_runtime, "/healthz", 200).unwrap();
    second_server.join().unwrap();

    let first_state: String = connection
        .query_row(
            "SELECT state FROM runtime_instances WHERE id = ?1",
            [&first_runtime],
            |row| row.get(0),
        )
        .unwrap();
    let second_state: String = connection
        .query_row(
            "SELECT state FROM runtime_instances WHERE id = ?1",
            [&second_runtime],
            |row| row.get(0),
        )
        .unwrap();
    let running_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_instances
             WHERE state = 'running' AND removed_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let previous_promotion =
        promote_internal_candidate(&mut connection, &first_runtime, "/healthz", 200).unwrap_err();
    assert_eq!(first_state, "stopped");
    assert_eq!(second_state, "running");
    assert_eq!(running_count, 1);
    assert!(matches!(
        previous_promotion,
        PromoteInternalCandidateError::InvalidRuntimeState { actual, .. }
            if actual == "stopped"
    ));
}

#[test]
fn unhealthy_candidate_fails_without_replacing_the_current_runtime() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let (first_endpoint, first_server) = server_with_statuses(&[200]);
    let first_runtime =
        add_verifying_candidate(&mut connection, "another", 'a', 'b', first_endpoint);
    promote_internal_candidate(&mut connection, &first_runtime, "/healthz", 200).unwrap();
    first_server.join().unwrap();

    let (candidate_endpoint, candidate_server) = server_with_statuses(&[503; 5]);
    let candidate =
        add_verifying_candidate(&mut connection, "another", 'c', 'd', candidate_endpoint);
    let error =
        promote_internal_candidate(&mut connection, &candidate, "/healthz", 200).unwrap_err();
    candidate_server.join().unwrap();

    assert!(matches!(
        error,
        PromoteInternalCandidateError::CandidateUnhealthy { .. }
    ));
    assert_eq!(
        runtime_and_deployment_state(&connection, &first_runtime).0,
        "running"
    );
    let candidate_state = runtime_and_deployment_state(&connection, &candidate);
    assert_eq!(candidate_state.0, "starting");
    assert_eq!(candidate_state.1, "failed");
    let failure: (String, String) = connection
        .query_row(
            "SELECT failure_code, failure_stage FROM deployments
             WHERE id = (SELECT deployment_id FROM runtime_instances WHERE id = ?1)",
            [&candidate],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        failure,
        ("health_check_failed".to_owned(), "verifying".to_owned())
    );
}

#[test]
fn refuses_public_application_before_health_check() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let endpoint: SocketAddr = "127.0.0.1:30001".parse().unwrap();
    let runtime_id = add_verifying_candidate(&mut connection, "valid", 'a', 'b', endpoint);

    let error =
        promote_internal_candidate(&mut connection, &runtime_id, "/healthz", 200).unwrap_err();

    assert!(matches!(
        error,
        PromoteInternalCandidateError::PublicApplication { .. }
    ));
}

fn add_verifying_candidate(
    connection: &mut rusqlite::Connection,
    fixture: &str,
    commit_character: char,
    runtime_character: char,
    endpoint: SocketAddr,
) -> String {
    let application =
        import_application(connection, &fixture_path(fixture), None, None, None).unwrap();
    let digest = format!("sha256:{}", commit_character.to_string().repeat(64));
    let artifact = OciArtifact::new("localhost/test", &digest).unwrap();
    let release = create_release(connection, &application.id, &artifact).unwrap();
    let deployment = create_deployment(
        connection,
        &application.id,
        &release.id,
        DeploymentType::Deploy,
    )
    .unwrap();
    advance_deployment(
        connection,
        deployment.id.as_str(),
        DeploymentTransition::Start,
    )
    .unwrap();
    let external_runtime_id = runtime_character.to_string().repeat(64);
    let runtime = register_candidate_runtime(
        connection,
        deployment.id.as_str(),
        &external_runtime_id,
        endpoint,
        8080,
    )
    .unwrap();
    advance_deployment(
        connection,
        deployment.id.as_str(),
        DeploymentTransition::RuntimeRunning,
    )
    .unwrap();
    runtime.id.to_string()
}

fn runtime_and_deployment_state(
    connection: &rusqlite::Connection,
    runtime_id: &str,
) -> (String, String, Option<String>) {
    connection
        .query_row(
            "SELECT runtime_instances.state, deployments.status, deployments.finished_at
             FROM runtime_instances
             JOIN deployments ON deployments.id = runtime_instances.deployment_id
             WHERE runtime_instances.id = ?1",
            [runtime_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

fn server_with_statuses(statuses: &[u16]) -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let endpoint = listener.local_addr().unwrap();
    let statuses = statuses.to_vec();
    let server = thread::spawn(move || {
        for status in statuses {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n");
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (endpoint, server)
}

fn read_request(stream: &mut TcpStream) {
    let mut request = Vec::new();
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let mut buffer = [0; 1024];
        let bytes_read = stream.read(&mut buffer).unwrap();
        assert_ne!(bytes_read, 0);
        request.extend_from_slice(&buffer[..bytes_read]);
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
