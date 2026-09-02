use std::ffi::OsStr;
use std::fs;
use std::process::{Command, Output};

use pneuma::adapters::database;

use crate::support::{
    DeploymentEnvironment, assert_command_succeeded, executable_path, run_pneuma,
    temporary_database_path,
};

#[test]
fn visibility_set_toggles_public_and_internal() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    let internal = run_visibility_command(&environment, "internal");
    assert_command_succeeded(&internal);
    let stdout = String::from_utf8_lossy(&internal.stdout);
    assert!(
        stdout.contains(&format!(
            "Visibility for {}: Internal",
            environment.application_name
        )),
        "unexpected stdout: {stdout}"
    );
    let connection = database::open(&environment.database_path).unwrap();
    let visibility: String = connection
        .query_row("SELECT desired_visibility FROM exposures", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(visibility, "internal");
    assert!(
        environment
            .managed_caddy_directory
            .read_dir()
            .unwrap()
            .next()
            .is_none(),
        "internal visibility must remove the Caddy fragment"
    );

    let public = run_visibility_command(&environment, "public");
    assert_command_succeeded(&public);
    let stdout = String::from_utf8_lossy(&public.stdout);
    assert!(
        stdout.contains(&format!(
            "Visibility for {}: Public",
            environment.application_name
        )),
        "unexpected stdout: {stdout}"
    );
    assert!(stdout.contains("Domain:"));
    let visibility: String = connection
        .query_row("SELECT desired_visibility FROM exposures", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(visibility, "public");
    assert!(
        environment
            .managed_caddy_directory
            .read_dir()
            .unwrap()
            .next()
            .is_some(),
        "public visibility must materialize the Caddy fragment"
    );
}

fn run_visibility_command(environment: &DeploymentEnvironment, visibility: &str) -> Output {
    run_visibility_command_with_curl_status(environment, visibility, 200)
}

fn run_visibility_command_with_curl_status(
    environment: &DeploymentEnvironment,
    visibility: &str,
    curl_status: u16,
) -> Output {
    run_visibility_command_with_options(environment, visibility, curl_status, None)
}

fn run_visibility_command_with_podman_port(
    environment: &DeploymentEnvironment,
    visibility: &str,
    podman_port: &str,
) -> Output {
    run_visibility_command_with_options(environment, visibility, 200, Some(podman_port))
}

fn run_visibility_command_with_options(
    environment: &DeploymentEnvironment,
    visibility: &str,
    curl_status: u16,
    podman_port: Option<&str>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
    command
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .env("PNEUMA_WORKSPACE_PATH", &environment.workspace_path)
        .env(
            "PNEUMA_CADDY_MANAGED_PATH",
            &environment.managed_caddy_directory,
        )
        .env("PNEUMA_CADDYFILE_PATH", &environment.caddyfile_path)
        .env("PATH", executable_path(&environment.fake_bin))
        .env("PNEUMA_FAKE_PORT", "30000")
        .env("PNEUMA_FAKE_CURL_LOG", environment.root.join("curl.log"))
        .env("PNEUMA_FAKE_CURL_STATUS", curl_status.to_string())
        .env("PNEUMA_ASSERT_CLOSED_DATABASE", &environment.database_path);
    if let Some(podman_port) = podman_port {
        command.env("PNEUMA_FAKE_PODMAN_PORT", podman_port);
    }
    command
        .args([
            "app",
            "visibility",
            "set",
            &environment.application_name,
            visibility,
        ])
        .output()
        .unwrap()
}

#[test]
fn visibility_set_internal_is_idempotent_without_domain() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let internal = run_visibility_command(&environment, "internal");
    assert_command_succeeded(&internal);
    let stdout = String::from_utf8_lossy(&internal.stdout);
    assert!(
        stdout.contains(&format!(
            "Visibility for {}: Internal",
            environment.application_name
        )),
        "unexpected stdout: {stdout}"
    );

    let second = run_visibility_command(&environment, "internal");
    assert_command_succeeded(&second);
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains(&format!(
            "Visibility for {}: Internal",
            environment.application_name
        )),
        "unexpected stdout: {stdout}"
    );

    let connection = database::open(&environment.database_path).unwrap();
    let domain: Option<String> = connection
        .query_row("SELECT domain FROM exposures", [], |row| row.get(0))
        .unwrap();
    assert!(domain.is_none(), "internal exposure must keep domain NULL");
}

#[test]
fn public_visibility_without_an_active_runtime_persists_failed_intent() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    assert_command_succeeded(&run_visibility_command(&environment, "internal"));

    let public = run_visibility_command(&environment, "public");
    assert!(!public.status.success());
    assert!(String::from_utf8_lossy(&public.stderr).contains("no active runtime"));

    assert_exposure_state(&environment, "public", "failed", Some("runtime_missing"));
}

fn assert_exposure_state(
    environment: &DeploymentEnvironment,
    visibility: &str,
    materialization_state: &str,
    error_code: Option<&str>,
) {
    let connection = database::open(&environment.database_path).unwrap();
    let exposure: (String, String, Option<String>) = connection
        .query_row(
            "SELECT desired_visibility, materialization_state, last_error_code FROM exposures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        exposure,
        (
            visibility.to_owned(),
            materialization_state.to_owned(),
            error_code.map(str::to_owned),
        )
    );
}

#[test]
fn public_visibility_without_a_domain_is_rejected_before_external_effects() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let public = run_visibility_command(&environment, "public");
    assert_eq!(
        public.status.code(),
        Some(4),
        "a required exposure domain is a recorded-state conflict"
    );
    assert!(
        String::from_utf8_lossy(&public.stderr).contains("requires a domain"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&public.stderr)
    );

    assert_exposure_state(&environment, "internal", "not_materialized", None);
    assert!(!environment.managed_caddy_directory.exists());
}

#[test]
fn public_visibility_fails_as_external_when_the_observed_endpoint_is_not_loopback() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    assert_command_succeeded(&run_visibility_command(&environment, "internal"));

    let public = run_visibility_command_with_podman_port(&environment, "public", "10.0.0.2:31000");
    assert_eq!(
        public.status.code(),
        Some(5),
        "a non-loopback observed endpoint is an external integration failure"
    );
    assert!(
        String::from_utf8_lossy(&public.stderr).contains("failed to observe runtime"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&public.stderr)
    );
}

#[test]
fn failed_public_health_restores_the_previous_fragment_and_keeps_public_intent() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    assert_command_succeeded(&run_visibility_command(&environment, "internal"));

    let application_id: String = database::open(&environment.database_path)
        .unwrap()
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    fs::create_dir_all(&environment.managed_caddy_directory).unwrap();
    let fragment = environment
        .managed_caddy_directory
        .join(format!("{application_id}.caddy"));
    fs::write(&fragment, "previous route\n").unwrap();

    let public = run_visibility_command_with_curl_status(&environment, "public", 503);
    assert!(!public.status.success());
    assert_eq!(fs::read_to_string(fragment).unwrap(), "previous route\n");
    assert_exposure_state(
        &environment,
        "public",
        "failed",
        Some("external_health_check_failed"),
    );
}

#[test]
fn lost_public_completion_cas_restores_the_fragment_and_is_not_success() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    assert_command_succeeded(&run_visibility_command(&environment, "internal"));

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

    let public = run_visibility_command(&environment, "public");
    assert!(!public.status.success());
    assert!(String::from_utf8_lossy(&public.stderr).contains("changed while"));
    assert_exposure_state(&environment, "public", "failed", Some("exposure_changed"));
    assert!(
        !environment
            .managed_caddy_directory
            .join(format!(
                "{}.caddy",
                database::open(&environment.database_path)
                    .unwrap()
                    .query_row("SELECT id FROM applications", [], |row| row
                        .get::<_, String>(0))
                    .unwrap()
            ))
            .exists(),
        "fragment must be restored after a lost completion CAS"
    );
}

#[test]
fn legacy_expose_command_returns_usage() {
    let database_path = temporary_database_path();
    let output = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("expose"),
            OsStr::new("personal-site"),
            OsStr::new("public"),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized subcommand"));
    assert!(stderr.contains("expose"));
}

#[test]
fn visibility_set_rejects_an_unknown_visibility() {
    let database_path = temporary_database_path();
    let output = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("visibility"),
            OsStr::new("set"),
            OsStr::new("personal-site"),
            OsStr::new("exposed"),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value"));
    assert!(stderr.contains("exposed"));
}
