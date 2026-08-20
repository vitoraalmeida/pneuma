use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::caddy_exposure::observe_caddy_fragment;
use pneuma::adapters::database;
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

    let input = load_reconciliation_input(&mut connection, "another").unwrap();

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
    let application_id = "1".repeat(32);

    assert_eq!(
        observe_caddy_fragment(&root, &application_id).unwrap(),
        CaddyFragmentObservation::Missing
    );
    assert!(!root.join(format!("{application_id}.caddy")).exists());

    fs::write(root.join(format!("{application_id}.caddy")), "route\n").unwrap();
    assert_eq!(
        observe_caddy_fragment(&root, &application_id).unwrap(),
        CaddyFragmentObservation::Present {
            contents: "route\n".to_owned()
        }
    );
    fs::remove_dir_all(root).unwrap();
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
