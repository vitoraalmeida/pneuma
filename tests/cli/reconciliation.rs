use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::thread;

use pneuma::adapters::application_lock::ApplicationLock;
use pneuma::adapters::database;

use crate::support::{DeploymentEnvironment, assert_command_succeeded, respond_once};

#[test]
fn reconcile_reports_no_op_for_stopped_intent_with_missing_resources() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    let connection = database::open(&environment.database_path).unwrap();
    connection
        .execute(
            "UPDATE applications SET desired_runtime_state = 'stopped' WHERE name = ?1",
            [&environment.application_name],
        )
        .unwrap();
    let before: (String, String) = connection
        .query_row(
            "SELECT desired_runtime_state, external_runtime_id
             FROM applications JOIN runtime_instances ON runtime_instances.application_id = applications.id
             WHERE applications.name = ?1",
            [&environment.application_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(connection);
    fs::write(environment.root.join("podman-removed"), "removed").unwrap();

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Application: another-site\nResult: no-op\n"
    );
    let connection = database::open(&environment.database_path).unwrap();
    let after: (String, String) = connection
        .query_row(
            "SELECT desired_runtime_state, external_runtime_id
             FROM applications JOIN runtime_instances ON runtime_instances.application_id = applications.id
             WHERE applications.name = ?1",
            [&environment.application_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn reconcile_reports_no_op_for_a_converged_running_application() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    // Deploy against the same endpoint reconcile will later observe.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    environment.reconciliation_port = Some(port);
    let output = environment.deploy(port, false);
    server.join().unwrap();
    assert_command_succeeded(&output);

    // Pin the fake's observed identity to the persisted runtime identity, the
    // way a real converged host would answer.
    let connection = database::open(&environment.database_path).unwrap();
    let recorded_id: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances WHERE removed_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    environment.replacement_container_id = Some(recorded_id);

    fs::remove_file(environment.root.join("podman.log")).unwrap();

    let connection = database::open(&environment.database_path).unwrap();
    let before: (String, String) = connection
        .query_row(
            "SELECT desired_runtime_state, active_deployment_id FROM applications WHERE name = ?1",
            [&environment.application_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(connection);

    let output = environment.run_reconcile();

    // Documented current precedence: only a confirmed public route reaches
    // no-op; a converged healthy INTERNAL application falls through to the
    // manual-intervention fallback (recorded deferred follow-up). The fallback
    // must not mutate any persisted intent or bookkeeping.
    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Application: another-site\nResult: manual-intervention\nDiagnostic: runtime identity or configuration differs from persisted intent\n"
    );
    let connection = database::open(&environment.database_path).unwrap();
    let after: (String, String) = connection
        .query_row(
            "SELECT desired_runtime_state, active_deployment_id FROM applications WHERE name = ?1",
            [&environment.application_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn reconcile_defers_before_external_observation_for_a_nonterminal_deployment() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    let connection = database::open(&environment.database_path).unwrap();
    let (application_id, release_id): (String, String) = connection
        .query_row(
            "SELECT applications.id, releases.id
             FROM applications JOIN releases ON releases.application_id = applications.id
             WHERE applications.name = ?1",
            [&environment.application_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let blocking_id = "d".repeat(32);
    connection
        .execute(
            "INSERT INTO deployments (id, application_id, release_id, type, status, requested_at)
             VALUES (?1, ?2, ?3, 'deploy', 'pending', CURRENT_TIMESTAMP)",
            [&blocking_id, &application_id, &release_id],
        )
        .unwrap();
    drop(connection);
    let podman_log = environment.root.join("podman.log");
    fs::remove_file(&podman_log).unwrap();
    let _lock = ApplicationLock::try_acquire(
        &environment.database_path,
        &pneuma::domain::identity::ApplicationId::new(application_id.as_str()).unwrap(),
    )
    .unwrap()
    .unwrap();

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Application: another-site\nResult: deferred\n"
    );
    assert!(
        !podman_log.exists(),
        "reconcile observed Podman while deferred"
    );
}

#[test]
fn reconcile_repairs_a_confirmed_quadlet_container_recreation() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let connection = database::open(&environment.database_path).unwrap();
    let (runtime_id, recorded_id, deployment_id, host_port): (String, String, String, u16) =
        connection
            .query_row(
                "SELECT runtime_instances.id, runtime_instances.external_runtime_id,
                    runtime_instances.deployment_id, runtime_instances.host_port
             FROM runtime_instances",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
    drop(connection);
    let replacement_id = "c".repeat(64);
    environment.stale_container_id = Some(recorded_id.clone());
    environment.replacement_container_id = Some(replacement_id.clone());

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Application: another-site\nResult: repaired\nRuntime: {runtime_id}\nContainer: {replacement_id}\n"
        )
    );
    let connection = database::open(&environment.database_path).unwrap();
    let (persisted_id, persisted_deployment, persisted_port, removed_at):
        (String, String, u16, Option<String>) = connection
        .query_row(
            "SELECT external_runtime_id, deployment_id, host_port, removed_at FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(persisted_id, replacement_id);
    assert_eq!(persisted_deployment, deployment_id);
    assert_eq!(persisted_port, host_port);
    assert!(removed_at.is_none());
}

#[test]
fn reconcile_rematerializes_a_missing_quadlet_and_container() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let connection = database::open(&environment.database_path).unwrap();
    let (runtime_id, deployment_id, host_port): (String, String, u16) = connection
        .query_row(
            "SELECT id, deployment_id, host_port FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    drop(connection);
    fs::remove_file(
        environment
            .root
            .join("quadlets")
            .join(format!("pneuma-another-site-{deployment_id}.container")),
    )
    .unwrap();
    fs::write(environment.root.join("podman-removed"), "removed\n").unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, host_port)).unwrap();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.run_reconcile();

    server.join().unwrap();
    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Result: repaired")
    );
    let connection = database::open(&environment.database_path).unwrap();
    let (persisted_runtime, persisted_port, removed_at): (String, u16, Option<String>) = connection
        .query_row(
            "SELECT id, host_port, removed_at FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(persisted_runtime, runtime_id);
    assert_eq!(persisted_port, host_port);
    assert!(removed_at.is_none());
}

#[test]
fn reconcile_refuses_a_rematerialized_container_that_differs_from_persisted_intent() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let connection = database::open(&environment.database_path).unwrap();
    let (deployment_id, recorded_id): (String, String) = connection
        .query_row(
            "SELECT deployment_id, external_runtime_id FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(connection);
    fs::remove_file(
        environment
            .root
            .join("quadlets")
            .join(format!("pneuma-another-site-{deployment_id}.container")),
    )
    .unwrap();
    fs::write(environment.root.join("podman-removed"), "removed\n").unwrap();
    environment.replacement_application_label = Some("other-app".to_owned());

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Result: manual-intervention"), "{stdout}");
    let connection = database::open(&environment.database_path).unwrap();
    let persisted_id: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_id, recorded_id);
}

#[test]
fn reconcile_reports_failure_when_the_rematerialized_runtime_fails_its_health_check() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let connection = database::open(&environment.database_path).unwrap();
    let (deployment_id, recorded_id): (String, String) = connection
        .query_row(
            "SELECT deployment_id, external_runtime_id FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(connection);
    fs::remove_file(
        environment
            .root
            .join("quadlets")
            .join(format!("pneuma-another-site-{deployment_id}.container")),
    )
    .unwrap();
    // No listener serves the reserved endpoint after rematerialization, so the
    // started container must fail its internal health check.
    fs::write(environment.root.join("podman-removed"), "removed\n").unwrap();

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Result: failed"), "{stdout}");
    assert!(
        stdout.contains("internal health check"),
        "unexpected diagnostic: {stdout}"
    );
    let connection = database::open(&environment.database_path).unwrap();
    let persisted_id: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_id, recorded_id);
}

#[test]
fn a_lost_runtime_confirmation_during_reconcile_surfaces_as_not_converged() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let connection = database::open(&environment.database_path).unwrap();
    let (deployment_id, host_port, recorded_id): (String, u16, String) = connection
        .query_row(
            "SELECT deployment_id, host_port, external_runtime_id FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_runtime_identity_swap
             BEFORE UPDATE OF external_runtime_id ON runtime_instances
             BEGIN
                 SELECT RAISE(IGNORE);
             END",
        )
        .unwrap();
    drop(connection);
    fs::remove_file(
        environment
            .root
            .join("quadlets")
            .join(format!("pneuma-another-site-{deployment_id}.container")),
    )
    .unwrap();
    fs::write(environment.root.join("podman-removed"), "removed\n").unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, host_port)).unwrap();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.run_reconcile();

    server.join().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("changed before rematerialization could be confirmed"),
        "unexpected stderr: {stderr}"
    );
    let connection = database::open(&environment.database_path).unwrap();
    let persisted_id: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_id, recorded_id);
}

#[test]
fn reconcile_restarts_a_canonical_quadlet_after_its_container_is_removed() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let connection = database::open(&environment.database_path).unwrap();
    let (runtime_id, deployment_id, host_port): (String, String, u16) = connection
        .query_row(
            "SELECT id, deployment_id, host_port FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    drop(connection);
    let quadlet_path = environment
        .root
        .join("quadlets")
        .join(format!("pneuma-another-site-{deployment_id}.container"));
    let original_unit = fs::read(&quadlet_path).unwrap();
    fs::write(environment.root.join("podman-removed"), "removed\n").unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, host_port)).unwrap();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.run_reconcile();

    server.join().unwrap();
    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Result: repaired")
    );
    assert_eq!(fs::read(quadlet_path).unwrap(), original_unit);
    let connection = database::open(&environment.database_path).unwrap();
    let (persisted_runtime, persisted_port, removed_at): (String, u16, Option<String>) = connection
        .query_row(
            "SELECT id, host_port, removed_at FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(persisted_runtime, runtime_id);
    assert_eq!(persisted_port, host_port);
    assert!(removed_at.is_none());
}

#[test]
fn reconcile_reports_manual_intervention_for_a_divergent_recreated_container() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let connection = database::open(&environment.database_path).unwrap();
    let recorded_id: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    environment.stale_container_id = Some(recorded_id.clone());
    environment.replacement_container_id = Some("c".repeat(64));
    environment.replacement_application_label = Some("other-app".to_owned());

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Result: manual-intervention")
    );
    let connection = database::open(&environment.database_path).unwrap();
    let persisted_id: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_id, recorded_id);
}

#[test]
fn reconcile_repairs_a_missing_public_caddy_fragment_with_configured_caddyfile() {
    let mut environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let fragment = environment.managed_caddy_directory.join(
        database::open(&environment.database_path)
            .unwrap()
            .query_row("SELECT id FROM applications", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            + ".caddy",
    );
    fs::remove_file(&fragment).unwrap();

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Application: personal-site\nResult: repaired\n"
    );
    assert!(fragment.exists());
    let connection = database::open(&environment.database_path).unwrap();
    let state: String = connection
        .query_row("SELECT materialization_state FROM exposures", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(state, "active");
}

#[test]
fn reconcile_records_failed_public_exposure_when_external_health_cannot_confirm_it() {
    let mut environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    environment.reconciliation_curl_status = Some(503);
    let application_id: String = database::open(&environment.database_path)
        .unwrap()
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    let fragment = environment
        .managed_caddy_directory
        .join(format!("{application_id}.caddy"));
    fs::remove_file(&fragment).unwrap();

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Result: failed")
    );
    assert!(
        !fragment.exists(),
        "failed repair must restore the missing fragment"
    );
    let connection = database::open(&environment.database_path).unwrap();
    let state: String = connection
        .query_row("SELECT materialization_state FROM exposures", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(state, "failed");
}

#[test]
fn reconcile_records_failed_public_exposure_when_caddy_rejects_the_materialization() {
    let mut environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let application_id: String = database::open(&environment.database_path)
        .unwrap()
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    let fragment = environment
        .managed_caddy_directory
        .join(format!("{application_id}.caddy"));
    fs::remove_file(&fragment).unwrap();
    fs::write(
        environment.fake_bin.join("caddy"),
        "#!/bin/sh\nprintf 'caddy rejected the configuration\\n' >&2\nexit 1\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(environment.fake_bin.join("caddy"))
        .unwrap()
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(environment.fake_bin.join("caddy"), permissions).unwrap();

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Result: failed")
    );
    assert!(
        !fragment.exists(),
        "a rejected materialization must not leave a fragment behind"
    );
    let (state, code): (String, String) = database::open(&environment.database_path)
        .unwrap()
        .query_row(
            "SELECT materialization_state, last_error_code FROM exposures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(code, "caddy_materialization_failed");
}

#[test]
fn a_lost_public_confirmation_restores_the_fragment_and_records_failure_during_reconcile() {
    let mut environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
    environment.reconciliation_port = Some(port);
    let application_id: String = database::open(&environment.database_path)
        .unwrap()
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    let fragment = environment
        .managed_caddy_directory
        .join(format!("{application_id}.caddy"));
    fs::remove_file(&fragment).unwrap();

    let connection = database::open(&environment.database_path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_public_exposure_completion
             BEFORE UPDATE OF active_runtime_id ON exposures
             BEGIN
                 SELECT RAISE(IGNORE);
             END",
        )
        .unwrap();
    drop(connection);

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Result: failed")
    );
    assert!(
        !fragment.exists(),
        "a lost confirmation CAS must restore the absent fragment"
    );
    let (state, code): (String, String) = database::open(&environment.database_path)
        .unwrap()
        .query_row(
            "SELECT materialization_state, last_error_code FROM exposures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(code, "exposure_changed");
}

#[test]
fn reconcile_removes_an_internal_caddy_fragment() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    let application_id: String = database::open(&environment.database_path)
        .unwrap()
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    fs::create_dir_all(&environment.managed_caddy_directory).unwrap();
    fs::write(
        environment
            .managed_caddy_directory
            .join(format!("{application_id}.caddy")),
        "unexpected route\n",
    )
    .unwrap();

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Application: another-site\nResult: repaired\n"
    );
    assert!(
        !environment
            .managed_caddy_directory
            .join(format!("{application_id}.caddy"))
            .exists()
    );
    let connection = database::open(&environment.database_path).unwrap();
    let state: String = connection
        .query_row("SELECT materialization_state FROM exposures", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(state, "not_materialized");
}

#[test]
fn lost_removal_completion_cas_restores_the_fragment_and_records_failure_during_reconcile() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    let application_id: String = database::open(&environment.database_path)
        .unwrap()
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    fs::create_dir_all(&environment.managed_caddy_directory).unwrap();
    let fragment = environment
        .managed_caddy_directory
        .join(format!("{application_id}.caddy"));
    fs::write(&fragment, "unexpected route\n").unwrap();

    let connection = database::open(&environment.database_path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_internal_exposure_completion
             BEFORE UPDATE OF active_runtime_id ON exposures
             BEGIN
                 SELECT RAISE(IGNORE);
             END",
        )
        .unwrap();
    drop(connection);

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Result: failed")
    );
    assert_eq!(
        fs::read_to_string(&fragment).unwrap(),
        "unexpected route\n",
        "a lost completion CAS must restore the removed fragment"
    );
    let (state, code): (String, String) = database::open(&environment.database_path)
        .unwrap()
        .query_row(
            "SELECT materialization_state, last_error_code FROM exposures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(code, "exposure_changed");
}

#[test]
fn reconcile_reports_manual_intervention_for_diverged_exposure_intent() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    let connection = database::open(&environment.database_path).unwrap();
    connection
        .execute(
            "UPDATE exposures SET materialization_state = 'diverged', last_error_code = 'recovery_failed', last_error_message = 'route origin is unknown'",
            [],
        )
        .unwrap();

    let output = environment.run_reconcile();

    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Result: manual-intervention")
    );
}
