use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::caddy_exposure::observe_caddy_fragment;
use pneuma::adapters::database;
use pneuma::domain::application::ApplicationName;
use pneuma::domain::identity::ApplicationId;
use pneuma::domain::reconciliation::{CaddyFragmentObservation, QuadletSourceObservation};
use pneuma::use_cases::reconciliation::load_reconciliation_input;

#[test]
fn reconciliation_observations_preserve_missing_external_resources() {
    assert_eq!(
        QuadletSourceObservation::Missing,
        QuadletSourceObservation::Missing
    );
    assert_eq!(
        CaddyFragmentObservation::Missing,
        CaddyFragmentObservation::Missing
    );
}

#[test]
fn loads_the_active_reconciliation_snapshot_without_writing_sqlite() {
    let root = temporary_directory();
    let database_path = root.join("pneuma.sqlite3");
    let mut connection = database::open(&database_path).unwrap();
    let application_id = "1".repeat(32);
    let release_id = "2".repeat(32);
    let deployment_id = "3".repeat(32);
    let runtime_id = "4".repeat(32);
    let digest = format!("sha256:{}", "a".repeat(64));
    let external_id = "b".repeat(64);
    connection
        .execute_batch(&format!(
            "INSERT INTO applications (id, name, desired_runtime_state, spec_version, created_at, updated_at)
             VALUES ('{application_id}', 'another', 'running', 3, '2026-01-01', '2026-01-01');
             INSERT INTO exposures (application_id, desired_visibility, domain, materialization_state, created_at, updated_at)
             VALUES ('{application_id}', 'internal', NULL, 'not_materialized', '2026-01-01', '2026-01-01');
             INSERT INTO releases (id, application_id, image_reference, image_repository, image_digest, created_at)
             VALUES ('{release_id}', '{application_id}', 'registry.example/team/another@{digest}', 'registry.example/team/another', '{digest}', '2026-01-01');
             INSERT INTO deployments (id, application_id, release_id, type, status, requested_at, started_at, finished_at)
             VALUES ('{deployment_id}', '{application_id}', '{release_id}', 'deploy', 'succeeded', '2026-01-01', '2026-01-01', '2026-01-01');
             INSERT INTO runtime_instances (id, application_id, deployment_id, external_runtime_id, state, host_address, host_port, container_port, last_observed_state, last_observed_at)
             VALUES ('{runtime_id}', '{application_id}', '{deployment_id}', '{external_id}', 'running', '127.0.0.1', 30000, 8080, 'running', '2026-01-01');
             UPDATE applications SET active_deployment_id = '{deployment_id}' WHERE id = '{application_id}';"
        ))
        .unwrap();
    let before = connection.total_changes();

    let input =
        load_reconciliation_input(&mut connection, &ApplicationName::new("another").unwrap())
            .unwrap();

    assert_eq!(input.application.id.as_str(), application_id);
    let active = input.active.unwrap();
    assert_eq!(active.deployment.id.as_str(), deployment_id);
    assert_eq!(active.release.id.as_str(), release_id);
    assert_eq!(active.release.artifact.digest(), digest);
    assert_eq!(active.runtime.unwrap().id.as_str(), runtime_id);
    assert!(input.blocking_deployment.is_none());
    assert!(input.exposure.is_some());
    assert_eq!(connection.total_changes(), before);
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn observes_caddy_fragment_without_creating_it() {
    let root = temporary_directory();
    let application_id = ApplicationId::from("1".repeat(32));

    assert_eq!(
        observe_caddy_fragment(&root, &application_id).unwrap(),
        CaddyFragmentObservation::Missing
    );
    assert!(
        !root
            .join(format!("{}.caddy", application_id.as_str()))
            .exists()
    );

    fs::write(
        root.join(format!("{}.caddy", application_id.as_str())),
        "route\n",
    )
    .unwrap();
    assert_eq!(
        observe_caddy_fragment(&root, &application_id).unwrap(),
        CaddyFragmentObservation::Present {
            contents: "route\n".to_owned()
        }
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reconcile_marks_an_interrupted_pending_deployment_failed_without_external_effects() {
    let root = temporary_directory();
    let database_path = root.join("pneuma.sqlite3");
    let connection = database::open(&database_path).unwrap();
    seed_interrupted_deployment(&connection, "pending", false);

    let output = reconcile_command(&root, &database_path).output().unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Result: failed"));
    assert_eq!(
        connection
            .query_row("SELECT status FROM deployments", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "failed"
    );
    assert_eq!(
        connection
            .query_row("SELECT failure_code FROM deployments", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "operation_interrupted"
    );
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reconcile_cleans_a_verified_candidate_only_after_unit_identity_is_proven() {
    let root = temporary_directory();
    let database_path = root.join("pneuma.sqlite3");
    let connection = database::open(&database_path).unwrap();
    seed_interrupted_deployment(&connection, "verifying", true);
    let quadlets = root.join("quadlets");
    fs::create_dir_all(&quadlets).unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    fs::write(
        quadlets.join(format!("pneuma-another-{}.container", "3".repeat(32))),
        pneuma::adapters::systemd_quadlet::canonical_unit_contents(
            "another",
            &"3".repeat(32),
            &format!("registry.example/team/another@{digest}"),
            8080,
            30000,
            &digest,
        ),
    )
    .unwrap();
    install_cleanup_commands(&root.join("bin"));

    let output = reconcile_command(&root, &database_path).output().unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Result: failed"));
    assert!(!quadlets.exists() || fs::read_dir(&quadlets).unwrap().next().is_none());
    assert_eq!(
        connection
            .query_row("SELECT status FROM deployments", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "failed"
    );
    assert!(
        connection
            .query_row(
                "SELECT removed_at IS NOT NULL FROM runtime_instances",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reconcile_marks_an_interrupted_activation_route_diverged_when_prior_route_is_unproven() {
    let root = temporary_directory();
    let database_path = root.join("pneuma.sqlite3");
    let connection = database::open(&database_path).unwrap();
    seed_interrupted_deployment(&connection, "activating", false);

    let output = reconcile_command(&root, &database_path).output().unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Result: diverged"));
    assert_eq!(
        connection
            .query_row("SELECT status FROM deployments", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "failed"
    );
    assert_eq!(
        connection
            .query_row("SELECT materialization_state FROM exposures", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "diverged"
    );
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

fn reconcile_command(root: &std::path::Path, database_path: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
    command
        .env("PNEUMA_DATABASE_PATH", database_path)
        .env("PNEUMA_CADDY_MANAGED_PATH", root.join("caddy"))
        .env("PNEUMA_CADDYFILE_PATH", root.join("Caddyfile"))
        .env("PNEUMA_QUADLET_DIR", root.join("quadlets"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                env::var("PATH").unwrap()
            ),
        )
        .args(["reconcile", "another"]);
    command
}

fn seed_interrupted_deployment(connection: &rusqlite::Connection, status: &str, runtime: bool) {
    let application_id = "1".repeat(32);
    let release_id = "2".repeat(32);
    let deployment_id = "3".repeat(32);
    let digest = format!("sha256:{}", "a".repeat(64));
    connection
        .execute_batch(&format!(
            "INSERT INTO applications (id, name, desired_runtime_state, spec_version, created_at, updated_at)
             VALUES ('{application_id}', 'another', 'running', 3, '2026-01-01', '2026-01-01');
             INSERT INTO exposures (application_id, desired_visibility, domain, materialization_state, created_at, updated_at)
             VALUES ('{application_id}', 'public', 'another.example', 'applying', '2026-01-01', '2026-01-01');
             INSERT INTO releases (id, application_id, image_reference, image_repository, image_digest, created_at)
             VALUES ('{release_id}', '{application_id}', 'registry.example/team/another@{digest}', 'registry.example/team/another', '{digest}', '2026-01-01');
             INSERT INTO deployments (id, application_id, release_id, type, status, requested_at, started_at)
             VALUES ('{deployment_id}', '{application_id}', '{release_id}', 'deploy', '{status}', '2026-01-01', '2026-01-01');"
        ))
        .unwrap();
    if runtime {
        connection
            .execute_batch(&format!(
                "INSERT INTO runtime_instances (id, application_id, deployment_id, external_runtime_id, state, host_address, host_port, container_port, last_observed_state, last_observed_at)
                 VALUES ('{}', '{application_id}', '{deployment_id}', '{}', 'starting', '127.0.0.1', 30000, 8080, 'running', '2026-01-01');",
                "4".repeat(32),
                "b".repeat(64),
            ))
            .unwrap();
    }
}

fn install_cleanup_commands(directory: &std::path::Path) {
    fs::create_dir_all(directory).unwrap();
    for command in ["podman", "systemctl"] {
        let path = directory.join(command);
        let contents = if command == "podman" {
            "#!/bin/sh\nif [ \"$1\" = container ] && [ \"$2\" = exists ]; then exit 1; fi\nexit 0\n"
        } else {
            "#!/bin/sh\nexit 0\n"
        };
        fs::write(&path, contents).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn temporary_directory() -> PathBuf {
    let root = env::temp_dir().join(format!(
        "pneuma-reconciliation-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
