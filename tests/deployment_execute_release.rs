use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pneuma::adapters::database;
use pneuma::domain::exposure::ExposureMaterialization;
use rusqlite::Connection;

#[test]
fn internal_deploy_succeeds_when_candidate_is_healthy() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 200));

    let output = environment.deploy(port, false);
    server.join().unwrap();

    assert_command_succeeded(&output);
    let deployment_id = extract_deployment_id(&output);
    let runtime_id = extract_runtime_id(&output);

    let connection = database::open(&environment.database_path).unwrap();

    let deployment_status: String = connection
        .query_row(
            "SELECT status FROM deployments WHERE id = ?1",
            [&deployment_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deployment_status, "succeeded");

    let app_id: String = connection
        .query_row(
            "SELECT id FROM applications WHERE name = ?1",
            [&environment.application_name],
            |row| row.get(0),
        )
        .unwrap();

    let active_deployment: String = connection
        .query_row(
            "SELECT active_deployment_id FROM applications WHERE id = ?1",
            [&app_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_deployment, deployment_id);

    let runtime_state: String = connection
        .query_row(
            "SELECT last_observed_state FROM runtime_instances WHERE id = ?1",
            [&runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(runtime_state, "running");

    let unit_path = environment
        .root
        .join("quadlets")
        .join(format!("pneuma-another-site-{deployment_id}.container"));
    assert!(unit_path.exists(), "unit file must exist");
    let unit = fs::read_to_string(unit_path).unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    assert!(unit.contains(&format!("Label=io.pneuma.image-digest={digest}")));
    assert!(!unit.contains("io.pneuma.revision="));
}

#[test]
fn deploy_fails_when_systemctl_start_fails() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    fs::write(environment.root.join("systemctl-start-failure"), "fail").unwrap();

    let output = environment.deploy_with_start_failure(30000);

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("runtime_start_failed"),
        "expected runtime_start_failed error, got: {stderr}"
    );

    let connection = database::open(&environment.database_path).unwrap();

    let deployment_status: String = connection
        .query_row(
            "SELECT status FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deployment_status, "failed");

    let failure_code: String = connection
        .query_row(
            "SELECT failure_code FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failure_code, "runtime_start_failed");

    let runtime_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_instances WHERE deployment_id IN (SELECT id FROM deployments ORDER BY requested_at DESC LIMIT 1)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(runtime_count, 0, "no runtime should be registered");

    let port_reservation_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_port_reservations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        port_reservation_count, 0,
        "port reservation must be released"
    );

    let unit_files: Vec<_> = fs::read_dir(environment.root.join("quadlets"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        unit_files.is_empty(),
        "unit file must be removed on failure"
    );
}

#[test]
fn deploy_fails_when_internal_health_check_fails() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 500));

    let output = environment.deploy(port, false);
    server.join().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("health_check_failed"),
        "expected health_check_failed error, got: {stderr}"
    );

    let connection = database::open(&environment.database_path).unwrap();

    let deployment_status: String = connection
        .query_row(
            "SELECT status FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deployment_status, "failed");

    let runtime_state: Option<String> = connection
        .query_row(
            "SELECT last_observed_state FROM runtime_instances ORDER BY last_observed_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();
    assert!(
        runtime_state.is_none() || runtime_state.as_deref() == Some("missing"),
        "candidate runtime must be marked as missing or not exist"
    );

    let port_reservation_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_port_reservations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        port_reservation_count, 0,
        "port reservation must be released"
    );

    let deployment_id: String = connection
        .query_row(
            "SELECT id FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let unit_path = environment
        .root
        .join("quadlets")
        .join(format!("pneuma-another-site-{deployment_id}.container"));
    assert!(
        !unit_path.exists(),
        "unit file must be removed when the candidate fails its health check"
    );
}

#[test]
fn new_deploy_removes_previous_runtime() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let first_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let first_port = first_listener.local_addr().unwrap().port();
    let first_server = thread::spawn(move || respond_until_timeout(&first_listener, 200));
    let first_output = environment.deploy(first_port, false);
    first_server.join().unwrap();
    assert_command_succeeded(&first_output);
    let first_runtime_id = extract_runtime_id(&first_output);
    let first_deployment_id = extract_deployment_id(&first_output);

    let second_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let second_port = second_listener.local_addr().unwrap().port();
    let second_server = thread::spawn(move || respond_until_timeout(&second_listener, 200));
    let second_output = environment.deploy_with_different_digest(second_port, 'b');
    second_server.join().unwrap();
    assert_command_succeeded(&second_output);
    let second_runtime_id = extract_runtime_id(&second_output);
    let second_deployment_id = extract_deployment_id(&second_output);

    let connection = database::open(&environment.database_path).unwrap();

    let app_id: String = connection
        .query_row(
            "SELECT id FROM applications WHERE name = ?1",
            [&environment.application_name],
            |row| row.get(0),
        )
        .unwrap();

    let first_runtime_removed: Option<String> = connection
        .query_row(
            "SELECT removed_at FROM runtime_instances WHERE id = ?1",
            [&first_runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        first_runtime_removed.is_some(),
        "previous runtime must be marked as removed"
    );

    let second_runtime_removed: Option<String> = connection
        .query_row(
            "SELECT removed_at FROM runtime_instances WHERE id = ?1",
            [&second_runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        second_runtime_removed.is_none(),
        "current runtime must not be removed"
    );

    let active_deployment: String = connection
        .query_row(
            "SELECT active_deployment_id FROM applications WHERE id = ?1",
            [&app_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_deployment, second_deployment_id);

    let first_unit = environment.root.join("quadlets").join(format!(
        "pneuma-another-site-{first_deployment_id}.container"
    ));
    assert!(!first_unit.exists(), "previous unit file must be removed");
}

#[test]
fn retirement_records_removal_only_after_the_container_is_observed_gone() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let first_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let first_port = first_listener.local_addr().unwrap().port();
    let first_server = thread::spawn(move || respond_until_timeout(&first_listener, 200));
    let first_output = environment.deploy(first_port, false);
    first_server.join().unwrap();
    assert_command_succeeded(&first_output);
    let first_runtime_id = extract_runtime_id(&first_output);

    // The fake removal is applied by Podman but never observable afterwards, so
    // destruction cannot be proven and retirement must not be recorded.
    fs::write(environment.root.join("podman-rm-ignored"), "ignore").unwrap();

    let second_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let second_port = second_listener.local_addr().unwrap().port();
    let second_server = thread::spawn(move || respond_until_timeout(&second_listener, 200));
    let second_output = environment.deploy_with_different_digest(second_port, 'b');
    second_server.join().unwrap();
    assert_command_succeeded(&second_output);
    let stderr = String::from_utf8_lossy(&second_output.stderr);
    assert!(
        stderr.contains("container removal could not be proven"),
        "expected an unproven removal warning, got: {stderr}"
    );

    let connection = database::open(&environment.database_path).unwrap();
    let first_runtime_removed: Option<String> = connection
        .query_row(
            "SELECT removed_at FROM runtime_instances WHERE id = ?1",
            [&first_runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        first_runtime_removed.is_none(),
        "retirement must not be recorded without observed container absence"
    );
}

#[test]
fn retirement_proves_a_quadlet_removed_container_without_forcing_it() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let first_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let first_port = first_listener.local_addr().unwrap().port();
    let first_server = thread::spawn(move || respond_until_timeout(&first_listener, 200));
    let first_output = environment.deploy(first_port, false);
    first_server.join().unwrap();
    assert_command_succeeded(&first_output);
    let first_runtime_id = extract_runtime_id(&first_output);

    // Quadlet's ExecStop removes the supervised container itself; pre-record the
    // previous container identity as already gone so retirement must adopt that
    // observation instead of force-removing a container that no longer exists.
    fs::write(
        environment.root.join("podman-removed-ids"),
        format!("{}\n", "a".repeat(64)),
    )
    .unwrap();

    let second_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let second_port = second_listener.local_addr().unwrap().port();
    let second_server = thread::spawn(move || respond_until_timeout(&second_listener, 200));
    let second_output = environment.deploy_with_different_digest(second_port, 'b');
    second_server.join().unwrap();
    assert_command_succeeded(&second_output);

    let podman_log = fs::read_to_string(environment.root.join("podman.log")).unwrap();
    assert!(
        podman_log.contains(&format!("container exists {}", "a".repeat(64))),
        "retirement must observe the previous container, log: {podman_log}"
    );
    assert!(
        !podman_log.contains(&format!("container rm --force {}", "a".repeat(64))),
        "an already removed container must never be force-removed, log: {podman_log}"
    );

    let connection = database::open(&environment.database_path).unwrap();
    let first_runtime_removed: Option<String> = connection
        .query_row(
            "SELECT removed_at FROM runtime_instances WHERE id = ?1",
            [&first_runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        first_runtime_removed.is_some(),
        "observed absence must be recorded as retirement"
    );
}

#[test]
fn failed_candidate_cleanup_requires_observed_container_removal() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.set_public_visibility();
    fs::write(environment.root.join("podman-rm-ignored"), "ignore").unwrap();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 200));

    let output = environment.deploy_with_external_status(port, false, 500);
    server.join().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("could not be cleaned up"),
        "expected a cleanup divergence, got: {stderr}"
    );

    let connection = database::open(&environment.database_path).unwrap();
    let (candidate_state, candidate_removed_at): (String, Option<String>) = connection
        .query_row(
            "SELECT state, removed_at FROM runtime_instances
             WHERE deployment_id = (SELECT id FROM deployments ORDER BY requested_at DESC LIMIT 1)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(candidate_state, "starting");
    assert!(
        candidate_removed_at.is_none(),
        "an unproven removal must never mark the candidate runtime missing"
    );
}

#[test]
fn failed_internal_verification_does_not_retire_the_healthy_previous_runtime() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let first_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let first_port = first_listener.local_addr().unwrap().port();
    let first_server = thread::spawn(move || respond_until_timeout(&first_listener, 200));
    let first_output = environment.deploy(first_port, false);
    first_server.join().unwrap();
    assert_command_succeeded(&first_output);
    let first_runtime_id = extract_runtime_id(&first_output);
    let first_deployment_id = extract_deployment_id(&first_output);

    let failing_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let failing_port = failing_listener.local_addr().unwrap().port();
    let failing_server = thread::spawn(move || respond_until_timeout(&failing_listener, 500));
    let output = environment.deploy_with_different_digest(failing_port, 'b');
    failing_server.join().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("health_check_failed"),
        "expected health_check_failed error, got: {stderr}"
    );

    let connection = database::open(&environment.database_path).unwrap();

    let app_id: String = connection
        .query_row(
            "SELECT id FROM applications WHERE name = ?1",
            [&environment.application_name],
            |row| row.get(0),
        )
        .unwrap();

    let (first_state, first_removed_at): (String, Option<String>) = connection
        .query_row(
            "SELECT last_observed_state, removed_at FROM runtime_instances WHERE id = ?1",
            [&first_runtime_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(first_state, "running");
    assert!(
        first_removed_at.is_none(),
        "a failed replacement must not retire the healthy previous runtime"
    );

    let active_deployment: String = connection
        .query_row(
            "SELECT active_deployment_id FROM applications WHERE id = ?1",
            [&app_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_deployment, first_deployment_id);

    let first_unit = environment.root.join("quadlets").join(format!(
        "pneuma-another-site-{first_deployment_id}.container"
    ));
    assert!(first_unit.exists(), "previous unit file must survive");

    let port_reservation_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_port_reservations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        port_reservation_count, 0,
        "the rejected candidate's port reservation must be released"
    );
}

#[test]
fn public_deploy_succeeds_with_caddy_and_external_health() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.set_public_visibility();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 200));

    let output = environment.deploy(port, false);
    server.join().unwrap();

    assert_command_succeeded(&output);

    let connection = database::open(&environment.database_path).unwrap();

    let deployment_status: String = connection
        .query_row(
            "SELECT status FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deployment_status, "succeeded");

    let app_id: String = connection
        .query_row(
            "SELECT id FROM applications WHERE name = ?1",
            [&environment.application_name],
            |row| row.get(0),
        )
        .unwrap();

    let exposure_state: String = connection
        .query_row(
            "SELECT materialization_state FROM exposures WHERE application_id = ?1",
            [&app_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exposure_state, "active");

    let configuration_version: String = connection
        .query_row(
            "SELECT configuration_version FROM exposures WHERE application_id = ?1",
            [&app_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        configuration_version,
        format!("vitoralmeida.tech {{\n    reverse_proxy 127.0.0.1:{port}\n}}\n")
    );
    assert!(matches!(
        pneuma::adapters::stores::exposure_store::load_exposure(
            &connection,
            &pneuma::domain::identity::ApplicationId::new(app_id.as_str()).unwrap(),
        ),
        Ok(Some(exposure)) if matches!(
            exposure.materialization(),
            ExposureMaterialization::Active { .. }
        )
    ));

    let caddy_fragments: Vec<_> = fs::read_dir(&environment.managed_caddy_directory)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        !caddy_fragments.is_empty(),
        "Caddy fragment must be created"
    );

    let curl_log = fs::read_to_string(environment.root.join("curl.log")).unwrap();
    assert!(
        curl_log.contains("--resolve"),
        "curl must be called with --resolve for external health check"
    );
}

#[test]
fn public_deploy_rolls_back_caddy_when_external_health_fails() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.set_public_visibility();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 200));

    let output = environment.deploy_with_external_status(port, false, 500);
    server.join().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("external_health_check_failed"),
        "expected external_health_check_failed error, got: {stderr}"
    );

    let connection = database::open(&environment.database_path).unwrap();

    let deployment_status: String = connection
        .query_row(
            "SELECT status FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deployment_status, "failed");

    let app_id: String = connection
        .query_row(
            "SELECT id FROM applications WHERE name = ?1",
            [&environment.application_name],
            |row| row.get(0),
        )
        .unwrap();

    let exposure_state: String = connection
        .query_row(
            "SELECT materialization_state FROM exposures WHERE application_id = ?1",
            [&app_id],
            |row| row.get(0),
        )
        .unwrap();
    // The adapter's route rollback succeeds in this environment, so the exposure
    // outcome must be a confirmed failure, not an unresolved divergence.
    assert_eq!(exposure_state, "failed");
}

#[test]
fn public_deploy_fails_when_internal_health_check_fails() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.set_public_visibility();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 500));

    let output = environment.deploy(port, false);
    server.join().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("health_check_failed"),
        "expected health_check_failed error, got: {stderr}"
    );

    let connection = database::open(&environment.database_path).unwrap();

    let (status, failure_code): (String, String) = connection
        .query_row(
            "SELECT status, failure_code FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(failure_code, "health_check_failed");

    // The exposure is only prepared after internal verification succeeds.
    let exposure_state: String = connection
        .query_row(
            "SELECT materialization_state FROM exposures WHERE desired_visibility = 'public'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exposure_state, "not_materialized");

    // Activation retains candidate resources so the centralized finalizer cleans them up.
    assert_candidate_resources_released(&environment, &connection);
}

#[test]
fn public_deploy_fails_when_caddy_rejects_route_reload() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.set_public_visibility();
    fs::write(environment.root.join("caddy-reload-failure"), "fail").unwrap();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 200));

    let output = environment.deploy(port, false);
    server.join().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("caddy_materialization_failed"),
        "expected caddy_materialization_failed error, got: {stderr}"
    );
    // The reload outage also breaks the adapter's own compensation, so the failure must
    // be recorded as divergence instead of a confirmed safe rollback.
    assert!(
        stderr.contains("recovery also failed"),
        "expected unconfirmed route recovery, got: {stderr}"
    );

    let connection = database::open(&environment.database_path).unwrap();

    let (status, failure_code): (String, String) = connection
        .query_row(
            "SELECT status, failure_code FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(failure_code, "caddy_materialization_failed");

    let (exposure_state, last_error_code): (String, String) = connection
        .query_row(
            "SELECT materialization_state, last_error_code FROM exposures WHERE desired_visibility = 'public'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(exposure_state, "diverged");
    assert_eq!(last_error_code, "caddy_materialization_failed");

    // The adapter's own rollback must leave no fragment behind.
    let fragments = count_directory_entries(&environment.managed_caddy_directory);
    assert_eq!(
        fragments, 0,
        "materialization rollback must remove the route"
    );

    let curl_log = fs::read_to_string(environment.root.join("curl.log")).unwrap_or_default();
    assert!(
        !curl_log.contains("--resolve"),
        "external verification must not run after materialization fails"
    );

    // Activation retains candidate resources so the centralized finalizer cleans them up.
    assert_candidate_resources_released(&environment, &connection);
}

#[test]
fn public_deploy_rolls_back_route_when_promotion_is_rejected() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.set_public_visibility();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 200));

    let gate_directory = environment.root.join("gates");
    let mut child = environment.deploy_gated_with_curl_delay(port, &gate_directory, "10");

    for gate in [
        "deployment.pending",
        "deployment.starting-registered",
        "deployment.verifying",
        "deployment.activating",
    ] {
        wait_for_path(&gate_directory.join(format!("{gate}.ready")));
        fs::write(gate_directory.join(format!("{gate}.release")), "go").unwrap();
    }

    // External verification now sleeps; once the fragment exists the run is between
    // materialization and promotion, so concurrent state changes become observable.
    wait_for_first_file(&environment.managed_caddy_directory);

    let connection = database::open(&environment.database_path).unwrap();
    let updated = connection
        .execute(
            "UPDATE exposures SET materialization_state = 'diverged'
             WHERE desired_visibility = 'public' AND materialization_state = 'applying'",
            [],
        )
        .unwrap();
    assert_eq!(updated, 1, "exposure must be applying during activation");

    let status = wait_for_child(&mut child);
    server.join().unwrap();

    assert!(!status.success());

    let (status_text, failure_code): (String, String) = connection
        .query_row(
            "SELECT status, failure_code FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status_text, "failed");
    assert_eq!(failure_code, "candidate_promotion_failed");

    let (exposure_state, last_error_code): (String, String) = connection
        .query_row(
            "SELECT materialization_state, last_error_code FROM exposures WHERE desired_visibility = 'public'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(exposure_state, "failed");
    assert_eq!(last_error_code, "candidate_promotion_failed");

    // The rejected promotion must roll the materialized route back.
    let fragments = count_directory_entries(&environment.managed_caddy_directory);
    assert_eq!(fragments, 0, "promotion rejection must roll the route back");

    // The candidate was still only Starting, so its resources are released by cleanup.
    assert_candidate_resources_released(&environment, &connection);
}

#[test]
fn rollback_executes_a_new_deployment_from_historical_provenance() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    // First release becomes historical provenance once the second is active.
    let first_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let first_port = first_listener.local_addr().unwrap().port();
    let first_server = thread::spawn(move || respond_until_timeout(&first_listener, 200));
    let first_output = environment.deploy(first_port, false);
    first_server.join().unwrap();
    assert_command_succeeded(&first_output);
    let first_deployment_id = extract_deployment_id(&first_output);

    let second_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let second_port = second_listener.local_addr().unwrap().port();
    let second_server = thread::spawn(move || respond_until_timeout(&second_listener, 200));
    let second_output = environment.deploy_with_different_digest(second_port, 'b');
    second_server.join().unwrap();
    assert_command_succeeded(&second_output);
    let second_deployment_id = extract_deployment_id(&second_output);
    let second_runtime_id = extract_runtime_id(&second_output);

    let rollback_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let rollback_port = rollback_listener.local_addr().unwrap().port();
    let rollback_server = thread::spawn(move || respond_until_timeout(&rollback_listener, 200));
    let output = environment.rollback(rollback_port, true);
    rollback_server.join().unwrap();

    assert_command_succeeded(&output);
    let rollback_stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(rollback_stderr.contains("pull image: started"));
    assert!(rollback_stderr.contains("retire previous runtime: completed"));
    let rollback_deployment_id = extract_deployment_id(&output);
    let rollback_runtime_id = extract_runtime_id(&output);

    let connection = database::open(&environment.database_path).unwrap();
    let app_id: String = connection
        .query_row(
            "SELECT id FROM applications WHERE name = ?1",
            [&environment.application_name],
            |row| row.get(0),
        )
        .unwrap();

    // The rollback is a NEW deployment of the first Release's provenance.
    let rollback_row: (String, String, String) = connection
        .query_row(
            "SELECT d.type, d.status, d.release_id
             FROM deployments d JOIN deployments prior ON prior.id = ?1
             WHERE d.id = ?2",
            [&first_deployment_id, &rollback_deployment_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(rollback_row.0, "rollback");
    assert_eq!(rollback_row.1, "succeeded");
    let original_release: String = connection
        .query_row(
            "SELECT release_id FROM deployments WHERE id = ?1",
            [&first_deployment_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rollback_row.2, original_release);

    // History is insert-only: prior rows keep their identity and status.
    for survived in [&first_deployment_id, &second_deployment_id] {
        let status: String = connection
            .query_row(
                "SELECT status FROM deployments WHERE id = ?1",
                [survived],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "succeeded", "{survived} must be untouched");
    }

    // The application confirms the rollback deployment as active.
    let active_deployment: String = connection
        .query_row(
            "SELECT active_deployment_id FROM applications WHERE id = ?1",
            [&app_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_deployment, rollback_deployment_id);

    // The replaced runtime is retired; the rollback runtime is live.
    let second_runtime_removed: Option<String> = connection
        .query_row(
            "SELECT removed_at FROM runtime_instances WHERE id = ?1",
            [&second_runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(second_runtime_removed.is_some());
    let rollback_runtime_state: String = connection
        .query_row(
            "SELECT last_observed_state FROM runtime_instances WHERE id = ?1",
            [&rollback_runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rollback_runtime_state, "running");

    // The candidate unit of the rollback exists under the stable name.
    let unit_path = environment.root.join("quadlets").join(format!(
        "pneuma-another-site-{rollback_deployment_id}.container"
    ));
    assert!(unit_path.exists(), "rollback unit file must exist");
}

#[test]
fn cleanup_does_not_remove_already_promoted_runtime() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_until_timeout(&listener, 200));
    let output = environment.deploy(port, false);
    server.join().unwrap();
    assert_command_succeeded(&output);

    let runtime_id = extract_runtime_id(&output);

    let connection = database::open(&environment.database_path).unwrap();

    let runtime_state: String = connection
        .query_row(
            "SELECT state FROM runtime_instances WHERE id = ?1",
            [&runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(runtime_state, "running");

    let removed_at: Option<String> = connection
        .query_row(
            "SELECT removed_at FROM runtime_instances WHERE id = ?1",
            [&runtime_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        removed_at.is_none(),
        "promoted runtime must not be marked as removed"
    );

    let unit_path = environment.root.join("quadlets").join(format!(
        "pneuma-another-site-{}.container",
        extract_deployment_id(&output)
    ));
    assert!(
        unit_path.exists(),
        "unit file for promoted runtime must exist"
    );
}

struct DeploymentEnvironment {
    root: PathBuf,
    repository_path: PathBuf,
    database_path: PathBuf,
    workspace_path: PathBuf,
    fake_bin: PathBuf,
    application_name: String,
    managed_caddy_directory: PathBuf,
    caddyfile_path: PathBuf,
    image_repository: String,
}

impl DeploymentEnvironment {
    fn new() -> Self {
        Self::from_fixture("another", "another-site")
    }

    fn from_fixture(fixture: &str, application_name: &str) -> Self {
        let root = env::temp_dir().join(format!(
            "pneuma-deploy-release-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let repository_path = root.join("repository");
        let database_path = root.join("pneuma.sqlite3");
        let workspace_path = root.join("workspaces");
        let fake_bin = root.join("bin");
        let managed_caddy_directory = root.join("caddy-applications");
        let caddyfile_path = root.join("Caddyfile");

        fs::create_dir_all(&repository_path).unwrap();
        fs::create_dir_all(&managed_caddy_directory).unwrap();
        fs::create_dir(&fake_bin).unwrap();
        fs::copy(
            fixture_path(fixture).join("pneuma.toml"),
            repository_path.join("pneuma.toml"),
        )
        .unwrap();
        fs::write(repository_path.join("Containerfile"), "FROM scratch\n").unwrap();
        initialize_repository(&repository_path);
        install_fake_podman(&fake_bin);
        install_fake_systemctl(&fake_bin);
        install_fake_caddy_and_curl(&fake_bin);
        fs::write(
            &caddyfile_path,
            format!("import {}/*.caddy\n", managed_caddy_directory.display()),
        )
        .unwrap();
        let manifest_content =
            fs::read_to_string(fixture_path(fixture).join("pneuma.toml")).unwrap();
        let image_repository = manifest_content
            .lines()
            .find(|line| line.starts_with("image = "))
            .map(|line| {
                line.trim_start_matches("image = ")
                    .trim_matches('"')
                    .to_owned()
            })
            .unwrap_or_else(|| "registry.example/team/service".to_owned());

        Self {
            root,
            repository_path,
            database_path,
            workspace_path,
            fake_bin,
            application_name: application_name.to_owned(),
            managed_caddy_directory,
            caddyfile_path,
            image_repository,
        }
    }

    fn public() -> Self {
        Self::from_fixture("valid", "personal-site")
    }

    fn deploy(&self, port: u16, verbose: bool) -> Output {
        self.deploy_with_external_status(port, verbose, 200)
    }

    fn deploy_with_external_status(
        &self,
        port: u16,
        verbose: bool,
        external_status: u16,
    ) -> Output {
        self.deploy_with_options(port, verbose, external_status, false)
    }

    fn deploy_with_options(
        &self,
        port: u16,
        verbose: bool,
        external_status: u16,
        systemctl_start_failure: bool,
    ) -> Output {
        self.deploy_command(port, verbose, external_status, systemctl_start_failure)
            .output()
            .unwrap()
    }

    fn deploy_command(
        &self,
        port: u16,
        verbose: bool,
        external_status: u16,
        systemctl_start_failure: bool,
    ) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
        command
            .env("PNEUMA_DATABASE_PATH", &self.database_path)
            .env("PNEUMA_WORKSPACE_PATH", &self.workspace_path)
            .env("PNEUMA_CADDY_MANAGED_PATH", &self.managed_caddy_directory)
            .env("PNEUMA_CADDYFILE_PATH", &self.caddyfile_path)
            .env("PNEUMA_QUADLET_DIR", self.root.join("quadlets"))
            .env("PATH", executable_path(&self.fake_bin))
            .env("PNEUMA_FAKE_PORT", port.to_string())
            .env("PNEUMA_RUNTIME_PORT_RANGE", format!("{port}-{port}"))
            .env("PNEUMA_FAKE_PODMAN_COUNT", self.root.join("podman-count"))
            .env("PNEUMA_FAKE_PODMAN_LOG", self.root.join("podman.log"))
            .env(
                "PNEUMA_FAKE_PODMAN_REMOVED_IDS",
                self.root.join("podman-removed-ids"),
            )
            .env(
                "PNEUMA_FAKE_PODMAN_RM_IGNORED",
                self.root.join("podman-rm-ignored"),
            )
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .env("PNEUMA_FAKE_CURL_STATUS", external_status.to_string())
            .env(
                "PNEUMA_FAKE_CADDY_RELOAD_FAILURE",
                self.root.join("caddy-reload-failure"),
            )
            .env(
                "PNEUMA_FAKE_PODMAN_DIGEST",
                format!("sha256:{}", "a".repeat(64)),
            );
        if systemctl_start_failure {
            command.env(
                "PNEUMA_FAKE_SYSTEMCTL_START_FAILURE",
                self.root.join("systemctl-start-failure"),
            );
        }
        if verbose {
            command.arg("--verbose");
        }
        let digest = format!("sha256:{}", "a".repeat(64));
        let reference = format!("{}@{digest}", self.image_repository);
        command.args([
            "app",
            "deploy",
            &self.application_name,
            "--image",
            &reference,
        ]);
        command
    }

    fn deploy_with_start_failure(&self, port: u16) -> Output {
        self.deploy_with_options(port, false, 200, true)
    }

    fn deploy_gated_with_curl_delay(
        &self,
        port: u16,
        gate_directory: &Path,
        curl_delay_seconds: &str,
    ) -> Child {
        let mut command = self.deploy_command(port, false, 200, false);
        command
            .env("PNEUMA_TEST_GATE_DIRECTORY", gate_directory)
            .env("PNEUMA_FAKE_CURL_DELAY", curl_delay_seconds)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().unwrap()
    }

    fn import(&self) -> Output {
        let repository_url = format!("file://{}", self.repository_path.display());
        Command::new(env!("CARGO_BIN_EXE_pneuma"))
            .env("PNEUMA_DATABASE_PATH", &self.database_path)
            .env("PNEUMA_WORKSPACE_PATH", &self.workspace_path)
            .args([
                OsStr::new("app"),
                OsStr::new("import"),
                OsStr::new(&repository_url),
            ])
            .output()
            .unwrap()
    }

    fn deploy_with_different_digest(&self, port: u16, digest_char: char) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
        command
            .env("PNEUMA_DATABASE_PATH", &self.database_path)
            .env("PNEUMA_WORKSPACE_PATH", &self.workspace_path)
            .env("PNEUMA_CADDY_MANAGED_PATH", &self.managed_caddy_directory)
            .env("PNEUMA_CADDYFILE_PATH", &self.caddyfile_path)
            .env("PNEUMA_QUADLET_DIR", self.root.join("quadlets"))
            .env("PATH", executable_path(&self.fake_bin))
            .env("PNEUMA_FAKE_PORT", port.to_string())
            .env("PNEUMA_RUNTIME_PORT_RANGE", format!("{port}-{port}"))
            .env("PNEUMA_FAKE_PODMAN_COUNT", self.root.join("podman-count"))
            .env("PNEUMA_FAKE_PODMAN_LOG", self.root.join("podman.log"))
            .env(
                "PNEUMA_FAKE_PODMAN_REMOVED_IDS",
                self.root.join("podman-removed-ids"),
            )
            .env(
                "PNEUMA_FAKE_PODMAN_RM_IGNORED",
                self.root.join("podman-rm-ignored"),
            )
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .env("PNEUMA_FAKE_CURL_STATUS", "200")
            .env(
                "PNEUMA_FAKE_PODMAN_DIGEST",
                format!("sha256:{}", digest_char.to_string().repeat(64)),
            );
        let digest = format!("sha256:{}", digest_char.to_string().repeat(64));
        let reference = format!("{}@{digest}", self.image_repository);
        command
            .args([
                "app",
                "deploy",
                &self.application_name,
                "--image",
                &reference,
            ])
            .output()
            .unwrap()
    }

    fn rollback(&self, port: u16, verbose: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
        command
            .env("PNEUMA_DATABASE_PATH", &self.database_path)
            .env("PNEUMA_WORKSPACE_PATH", &self.workspace_path)
            .env("PNEUMA_CADDY_MANAGED_PATH", &self.managed_caddy_directory)
            .env("PNEUMA_CADDYFILE_PATH", &self.caddyfile_path)
            .env("PNEUMA_QUADLET_DIR", self.root.join("quadlets"))
            .env("PATH", executable_path(&self.fake_bin))
            // The fake Podman answers every endpoint observation with this port.
            .env("PNEUMA_FAKE_PORT", port.to_string())
            // A distinct container identity for the rollback candidate, since
            // the fake would otherwise reuse the previous deployment's ID.
            .env("PNEUMA_FAKE_PODMAN_ID", "c".repeat(64))
            .env(
                "PNEUMA_FAKE_PODMAN_REMOVED_IDS",
                self.root.join("podman-removed-ids"),
            )
            .env(
                "PNEUMA_FAKE_PODMAN_RM_IGNORED",
                self.root.join("podman-rm-ignored"),
            );
        if verbose {
            command.arg("--verbose");
        }
        command
            .args(["deployment", "rollback", &self.application_name])
            .output()
            .unwrap()
    }

    fn set_public_visibility(&self) {
        let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
            .env("PNEUMA_DATABASE_PATH", &self.database_path)
            .env("PNEUMA_WORKSPACE_PATH", &self.workspace_path)
            .env("PNEUMA_CADDY_MANAGED_PATH", &self.managed_caddy_directory)
            .env("PNEUMA_CADDYFILE_PATH", &self.caddyfile_path)
            .env("PATH", executable_path(&self.fake_bin))
            .env("PNEUMA_FAKE_PORT", "30000")
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .args(["app", "visibility", "set", &self.application_name, "public"])
            .output()
            .unwrap();
        assert_command_succeeded(&output);
    }
}

impl Drop for DeploymentEnvironment {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn initialize_repository(repository_path: &Path) {
    let output = Command::new("git")
        .args(["init", "--quiet", "--initial-branch=main"])
        .arg(repository_path)
        .output()
        .unwrap();
    assert_command_succeeded(&output);
    git(repository_path, &["add", "."]);
    git(
        repository_path,
        &[
            "-c",
            "user.name=Pneuma Tests",
            "-c",
            "user.email=pneuma@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "initial revision",
        ],
    );
}

fn assert_command_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git(repository_path: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args(arguments)
        .output()
        .unwrap();
    assert_command_succeeded(&output);
    String::from_utf8(output.stdout).unwrap()
}

fn respond_until_timeout(listener: &TcpListener, status: u16) {
    listener.set_nonblocking(true).unwrap();
    let timeout = Duration::from_secs(2);
    let start = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                read_request(&mut stream);
                let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n");
                stream.write_all(response.as_bytes()).unwrap();
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() > timeout {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
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

fn wait_for_first_file(directory: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if count_directory_entries(directory) > 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a file in {}",
            directory.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn count_directory_entries(directory: &Path) -> usize {
    fs::read_dir(directory)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .count()
}

fn install_fake_podman(fake_bin: &Path) {
    let podman = fake_bin.join("podman");
    fs::write(
        &podman,
        r#"#!/bin/sh
set -eu

if [ -n "${PNEUMA_FAKE_PODMAN_LOG:-}" ]; then
    printf '%s\n' "$*" >> "$PNEUMA_FAKE_PODMAN_LOG"
fi

case "$1" in
    build)
        ;;
    pull)
        if [ -f "${PNEUMA_FAKE_PODMAN_PULL_FAILURE:-}" ]; then
            printf 'pull failed\n' >&2
            exit 1
        fi
        ;;
    image)
        if [ "$2" = "inspect" ] && [ "$3" = "--format" ] && [ "$4" = "{{.Digest}}" ]; then
            if [ -n "${PNEUMA_FAKE_PODMAN_DIGEST:-}" ]; then
                printf '%s\n' "$PNEUMA_FAKE_PODMAN_DIGEST"
            else
                printf 'sha256:%s\n' "$(printf 'a%.0s' $(seq 1 64))"
            fi
        else
            printf 'unsupported fake Podman command: %s\n' "$*" >&2
            exit 1
        fi
        ;;
    container)
        if [ "$2" = "exists" ]; then
            if [ -f "${PNEUMA_FAKE_PODMAN_REMOVED:-}" ]; then
                exit 1
            fi
            if [ -n "${PNEUMA_FAKE_PODMAN_STALE_ID:-}" ] && [ "$3" = "$PNEUMA_FAKE_PODMAN_STALE_ID" ]; then
                exit 1
            fi
            if [ -f "${PNEUMA_FAKE_PODMAN_REMOVED_IDS:-}" ] && grep -qxF "$3" "$PNEUMA_FAKE_PODMAN_REMOVED_IDS"; then
                exit 1
            fi
        elif [ "$2" = "rm" ]; then
            if [ ! -f "${PNEUMA_FAKE_PODMAN_RM_IGNORED:-/nonexistent}" ] && [ -n "${PNEUMA_FAKE_PODMAN_REMOVED_IDS:-}" ]; then
                printf '%s\n' "$4" >> "$PNEUMA_FAKE_PODMAN_REMOVED_IDS"
            fi
        fi
        ;;
    create)
        count=0
        if [ -f "$PNEUMA_FAKE_PODMAN_COUNT" ]; then
            count=$(sed -n '1p' "$PNEUMA_FAKE_PODMAN_COUNT")
        fi
        count=$((count + 1))
        printf '%s\n' "$count" > "$PNEUMA_FAKE_PODMAN_COUNT"
        if [ "$count" -eq 1 ]; then
            printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'
        else
            printf 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n'
        fi
        ;;
    inspect)
        if [ "$2" = "--format" ] && [ "$3" = "{{.Id}}" ]; then
            if [ -f "${PNEUMA_FAKE_PODMAN_REMOVED:-}" ]; then
                exit 1
            fi
            if [ -n "${PNEUMA_FAKE_PODMAN_ID:-}" ]; then
                printf '%s\n' "$PNEUMA_FAKE_PODMAN_ID"
            else
                count=0
                if [ -n "${PNEUMA_FAKE_PODMAN_COUNT:-}" ] && [ -f "$PNEUMA_FAKE_PODMAN_COUNT" ]; then
                    count=$(sed -n '1p' "$PNEUMA_FAKE_PODMAN_COUNT")
                fi
                count=$((count + 1))
                if [ -n "${PNEUMA_FAKE_PODMAN_COUNT:-}" ]; then
                    printf '%s\n' "$count" > "$PNEUMA_FAKE_PODMAN_COUNT"
                fi
                if [ "$count" -eq 1 ]; then
                    printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'
                else
                    printf 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n'
                fi
            fi
        elif [ -n "${PNEUMA_FAKE_CONTAINER_STATE:-}" ] && [ -f "$PNEUMA_FAKE_CONTAINER_STATE" ]; then
            sed -n '1p' "$PNEUMA_FAKE_CONTAINER_STATE"
        else
            printf 'running\n'
        fi
        ;;
    port)
        printf '127.0.0.1:%s\n' "$PNEUMA_FAKE_PORT"
        ;;
    start)
        if [ -n "${PNEUMA_FAKE_CONTAINER_STATE:-}" ]; then
            printf 'running\n' > "$PNEUMA_FAKE_CONTAINER_STATE"
        fi
        ;;
    stop)
        if [ -n "${PNEUMA_FAKE_CONTAINER_STATE:-}" ]; then
            printf 'stopped\n' > "$PNEUMA_FAKE_CONTAINER_STATE"
        fi
        ;;
    *)
        printf 'unsupported fake Podman command: %s\n' "$*" >&2
        exit 1
        ;;
esac
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&podman).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(podman, permissions).unwrap();
}

fn install_fake_systemctl(fake_bin: &Path) {
    let systemctl = fake_bin.join("systemctl");
    fs::write(
        &systemctl,
        r#"#!/bin/sh
set -eu
if [ "$1" = "--user" ]; then
    shift
fi
case "$1" in
    daemon-reload|start|stop|enable|disable)
        if [ "$1" = "start" ] && [ -f "${PNEUMA_FAKE_SYSTEMCTL_START_FAILURE:-}" ]; then
            printf 'start failed\n' >&2
            exit 1
        fi
        if [ "$1" = "start" ] && [ -n "${PNEUMA_FAKE_CONTAINER_STATE:-}" ]; then
            printf 'running\n' > "$PNEUMA_FAKE_CONTAINER_STATE"
        fi
        if [ "$1" = "stop" ] && [ -n "${PNEUMA_FAKE_CONTAINER_STATE:-}" ]; then
            printf 'stopped\n' > "$PNEUMA_FAKE_CONTAINER_STATE"
        fi
        ;;
    *)
        printf 'unsupported fake systemctl command: %s\n' "$*" >&2
        exit 1
        ;;
esac
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&systemctl).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(systemctl, permissions).unwrap();
}

fn install_fake_caddy_and_curl(fake_bin: &Path) {
    for (name, script) in [
        (
            "caddy",
            "#!/bin/sh\nset -eu\ncase \"$1\" in\nvalidate) printf 'valid configuration\\n' ;;\nreload)\nif [ -f \"${PNEUMA_FAKE_CADDY_RELOAD_FAILURE:-}\" ]; then\nprintf 'reload failed\\n' >&2\nexit 1\nfi\nprintf 'reload complete\\n' ;;\n*) exit 1 ;;\nesac\n",
        ),
        (
            "curl",
            "#!/bin/sh\nset -eu\nif [ -n \"${PNEUMA_FAKE_CURL_DELAY:-}\" ]; then\nsleep \"$PNEUMA_FAKE_CURL_DELAY\"\nfi\nprintf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_CURL_LOG\"\nprintf '%s' \"${PNEUMA_FAKE_CURL_STATUS:-200}\"\n",
        ),
    ] {
        let executable = fake_bin.join(name);
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).unwrap();
    }
}

fn executable_path(fake_bin: &Path) -> std::ffi::OsString {
    let inherited = env::var_os("PATH").unwrap_or_default();
    env::join_paths(std::iter::once(fake_bin.to_path_buf()).chain(env::split_paths(&inherited)))
        .unwrap()
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_child(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the deployment process"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

// Verifies the centralized finalizer released every retained candidate resource.
fn assert_candidate_resources_released(
    environment: &DeploymentEnvironment,
    connection: &Connection,
) {
    let deployment_id: String = connection
        .query_row(
            "SELECT id FROM deployments ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let unit_path = environment.root.join("quadlets").join(format!(
        "pneuma-{}-{deployment_id}.container",
        environment.application_name
    ));
    assert!(
        !unit_path.exists(),
        "candidate unit must be removed by centralized cleanup"
    );

    let port_reservation_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_port_reservations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        port_reservation_count, 0,
        "port reservation must be released"
    );

    let runtime_state: String = connection
        .query_row(
            "SELECT last_observed_state FROM runtime_instances ORDER BY last_observed_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        runtime_state, "missing",
        "rejected candidate runtime must be marked missing"
    );
}

fn extract_deployment_id(output: &Output) -> String {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Deployment: "))
        .map(|s| s.to_owned())
        .expect("deployment id not found in output")
}

fn extract_runtime_id(output: &Output) -> String {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Runtime: "))
        .map(|s| s.to_owned())
        .expect("runtime id not found in output")
}
