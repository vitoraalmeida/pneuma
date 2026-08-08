use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::database;

#[test]
fn imports_and_lists_an_application_idempotently() {
    let database_path = temporary_database_path();
    let repository_path = fixture_path("valid");

    let first_import = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    );
    let second_import = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    );
    let list = run_pneuma(&database_path, &[OsStr::new("app"), OsStr::new("list")]);
    let _ = fs::remove_file(&database_path);

    assert!(first_import.status.success());
    assert!(second_import.status.success());
    assert_eq!(
        String::from_utf8_lossy(&first_import.stdout),
        "Imported personal-site\nStatus: Registered\nDeployment: Not deployed\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&list.stdout),
        "personal-site\tRegistered\tNot deployed\n"
    );
}

#[test]
fn reports_manifest_errors_and_returns_failure() {
    let database_path = temporary_database_path();
    let repository_path = fixture_path("missing");

    let output = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing/pneuma.toml"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reports_database_open_errors_and_returns_failure() {
    let database_path = temporary_database_path()
        .join("missing")
        .join("pneuma.sqlite3");

    let output = run_pneuma(&database_path, &[OsStr::new("app"), OsStr::new("list")]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to open database at"));
    assert!(stderr.contains(database_path.to_string_lossy().as_ref()));
}

#[test]
fn doctor_returns_failure_when_a_check_fails() {
    let database_path = temporary_database_path();
    let missing_workspace = temporary_database_path().join("missing");

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .env("PNEUMA_WORKSPACE_PATH", &missing_workspace)
        .args([OsStr::new("doctor")])
        .output()
        .unwrap();
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("one or more diagnostic checks failed"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reports_usage_for_an_unknown_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .args(["unknown"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("[--verbose]"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("[--verbose] app import"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("[--verbose] app list"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("[--verbose] app deploy"));
}

#[test]
fn deploys_an_internal_application_and_prints_its_identity() {
    let environment = DeploymentEnvironment::new();
    let import = environment.import();
    assert_command_succeeded(&import);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.deploy(port, false);
    server.join().unwrap();

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Deploying another-site...\n"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[0], "Deployed another-site");
    assert_eq!(lines[1], format!("Commit: {}", environment.commit_sha));
    assert_identifier_line(lines[2], "Deployment: ");
    assert_identifier_line(lines[3], "Runtime: ");
    assert!(lines[4].starts_with("Container: pneuma-another-site-"));
    assert_eq!(lines[5], "Status: Succeeded");
    assert_eq!(
        fs::read_dir(&environment.workspace_path).unwrap().count(),
        1
    );
    let connection = database::open(&environment.database_path).unwrap();
    let desired_state: String = connection
        .query_row(
            "SELECT desired_runtime_state FROM applications",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(desired_state, "running");
}

#[test]
fn deploy_writes_and_enables_a_per_deployment_quadlet_unit() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.deploy(port, false);
    server.join().unwrap();
    assert_command_succeeded(&output);
    let deployment_id = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Deployment: "))
        .unwrap()
        .to_owned();
    let unit = environment
        .root
        .join("quadlets")
        .join(format!("pneuma-another-site-{deployment_id}.container"));
    let content = fs::read_to_string(unit).unwrap();
    assert!(content.contains(&format!("PublishPort=127.0.0.1:{port}:8080")));
    assert!(content.contains("Restart=on-failure"));
    assert!(content.contains("WantedBy=default.target"));
}

#[test]
fn database_backup_and_restore_preserve_catalog_state() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let backup = environment.root.join("database-backup.sqlite3");
    let backup_output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .args(["database", "backup"])
        .arg(&backup)
        .output()
        .unwrap();
    assert_command_succeeded(&backup_output);
    let connection = database::open(&environment.database_path).unwrap();
    connection.execute("DELETE FROM applications", []).unwrap();
    drop(connection);
    let restore_output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .args(["database", "restore"])
        .arg(&backup)
        .output()
        .unwrap();
    assert_command_succeeded(&restore_output);
    let connection = database::open(&environment.database_path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    assert!(fs::read_dir(&environment.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("pre-restore")
    }));
}

#[test]
fn deploys_a_verified_oci_image_and_persists_its_release() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/team/service@{digest}");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.deploy_oci(&reference, port);
    server.join().unwrap();

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Deploying another-site...\n"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!("Image: {reference}\n")));
    assert!(!stdout.contains("Commit:"));
    let connection = database::open(&environment.database_path).unwrap();
    let release: (String, String, String, Option<String>) = connection
        .query_row(
            "SELECT image_reference, image_repository, image_digest, source_revision FROM releases",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        release,
        (
            reference.clone(),
            "registry.example/team/service".to_owned(),
            digest.clone(),
            None,
        )
    );
    let commands: Vec<String> = fs::read_to_string(environment.root.join("podman.log"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(commands[0], format!("pull {reference}"));
    assert_eq!(
        commands[1],
        format!("image inspect --format {{{{.Digest}}}} {reference}")
    );
    assert!(commands[2].starts_with("inspect --format {{.Id}} pneuma-another-site-"));
}

#[test]
fn rejects_a_mutable_oci_tag_without_podman_work() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let output = environment.deploy_oci("registry.example/team/service:latest", 30000);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be"));
    assert!(!environment.root.join("podman.log").exists());
}

#[test]
fn rejects_an_oci_repository_not_allowed_by_the_delivery_spec() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/other/service@{digest}");

    let output = environment.deploy_oci(&reference, 30000);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("only accepts images from"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!environment.root.join("podman.log").exists());
}

#[test]
fn oci_only_application_rejects_deploy_source() {
    let environment = DeploymentEnvironment::from_fixture("oci-only", "oci-only-app");
    assert_command_succeeded(&environment.import());

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .env("PNEUMA_WORKSPACE_PATH", &environment.workspace_path)
        .env(
            "PNEUMA_CADDY_MANAGED_PATH",
            &environment.managed_caddy_directory,
        )
        .env("PNEUMA_CADDYFILE_PATH", &environment.caddyfile_path)
        .env("PATH", executable_path(&environment.fake_bin))
        .env("PNEUMA_FAKE_PORT", "30000")
        .env(
            "PNEUMA_FAKE_PODMAN_LOG",
            environment.root.join("podman.log"),
        )
        .args(["app", "deploy-source", &environment.application_name])
        .arg(&environment.repository_path)
        .args(["--revision", "HEAD"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("has no [source]/[build] configuration"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!environment.root.join("podman.log").exists());
}

#[test]
fn oci_pull_and_digest_failures_create_no_release_or_runtime() {
    for failure in [OciFailure::Pull, OciFailure::DigestMismatch] {
        let environment = DeploymentEnvironment::new();
        assert_command_succeeded(&environment.import());
        let digest = format!("sha256:{}", "a".repeat(64));
        let reference = format!("registry.example/team/service@{digest}");

        let output = environment.deploy_oci_with_failure(&reference, failure);

        assert!(!output.status.success());
        let connection = database::open(&environment.database_path).unwrap();
        for table in ["releases", "deployments", "runtime_instances"] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} must remain empty after {failure:?}");
        }
        let commands = fs::read_to_string(environment.root.join("podman.log")).unwrap();
        match failure {
            OciFailure::Pull => assert_eq!(commands, format!("pull {reference}\n")),
            OciFailure::DigestMismatch => {
                assert!(commands.starts_with(&format!("pull {reference}\n")));
                assert!(commands.contains("image inspect --format {{.Digest}}"));
                assert!(!commands.contains("create "));
            }
        }
    }
}

#[test]
fn source_deploy_is_explicit_and_old_deploy_syntax_is_usage() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    let old_syntax = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .args(["app", "deploy", &environment.application_name])
        .arg(&environment.repository_path)
        .args(["--revision", "HEAD"])
        .output()
        .unwrap();

    assert!(!old_syntax.status.success());
    assert!(String::from_utf8_lossy(&old_syntax.stderr).contains("Usage:"));
}

#[test]
fn a_failed_deploy_retry_reuses_the_checkout() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let failing_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let failing_port = failing_listener.local_addr().unwrap().port();
    let failing_server = thread::spawn(move || respond_unhealthy(&failing_listener, 5));
    let first = environment.deploy(failing_port, false);
    failing_server.join().unwrap();
    assert!(!first.status.success());

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy(port, false));
    server.join().unwrap();
}

#[test]
fn reports_a_missing_application_before_external_work() {
    let database_path = temporary_database_path();
    let workspace_path = database_path.with_extension("workspaces");

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .env("PNEUMA_WORKSPACE_PATH", &workspace_path)
        .args([
            "app",
            "deploy-source",
            "missing-application",
            "missing-repository",
            "--revision",
            "main",
        ])
        .output()
        .unwrap();
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("application `missing-application` was not found")
    );
    assert!(!workspace_path.exists());
}

#[test]
fn deploys_a_public_application_and_persists_the_active_route() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.deploy(port, true);
    server.join().unwrap();

    assert_command_succeeded(&output);
    let curl_command = fs::read_to_string(environment.root.join("curl.log")).unwrap();
    assert!(curl_command.contains("--retry 30"));
    assert!(curl_command.contains("--resolve vitoralmeida.tech:443:127.0.0.1"));
    assert!(curl_command.contains("https://vitoralmeida.tech/healthz"));
    let connection = database::open(&environment.database_path).unwrap();
    let state: (String, String, String, String) = connection
        .query_row(
            "SELECT exposures.materialization_state,
                    exposures.configuration_version,
                     runtime_instances.state,
                    deployments.status
             FROM exposures
             JOIN runtime_instances ON runtime_instances.id = exposures.active_runtime_id
             JOIN deployments ON deployments.id = runtime_instances.deployment_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        state,
        (
            "active".to_owned(),
            environment.commit_sha.clone(),
            "running".to_owned(),
            "succeeded".to_owned(),
        )
    );
    let desired_state: String = connection
        .query_row(
            "SELECT desired_runtime_state FROM applications",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(desired_state, "running");
    assert!(
        environment
            .managed_caddy_directory
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
}

#[test]
fn restores_the_previous_public_route_when_external_health_fails() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    let first_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let first_port = first_listener.local_addr().unwrap().port();
    let first_server = thread::spawn(move || respond_once(&first_listener, 200));
    assert_command_succeeded(&environment.deploy(first_port, false));
    first_server.join().unwrap();
    let connection = database::open(&environment.database_path).unwrap();
    let first_active_runtime: String = connection
        .query_row("SELECT active_runtime_id FROM exposures", [], |row| {
            row.get(0)
        })
        .unwrap();
    let application_id: String = connection
        .query_row("SELECT application_id FROM exposures", [], |row| row.get(0))
        .unwrap();
    let fragment_path = environment
        .managed_caddy_directory
        .join(format!("{application_id}.caddy"));
    let first_fragment = fs::read_to_string(&fragment_path).unwrap();
    drop(connection);
    environment.commit("second revision");
    let second_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let second_port = second_listener.local_addr().unwrap().port();
    let second_server = thread::spawn(move || respond_once(&second_listener, 200));

    let output = environment.deploy_with_external_status(second_port, false, 503);
    second_server.join().unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("external_health_check_failed"));
    assert_eq!(fs::read_to_string(fragment_path).unwrap(), first_fragment);
    let connection = database::open(&environment.database_path).unwrap();
    let exposure: (String, String, String) = connection
        .query_row(
            "SELECT materialization_state, active_runtime_id, last_error_code
             FROM exposures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        exposure,
        (
            "failed".to_owned(),
            first_active_runtime.clone(),
            "external_health_check_failed".to_owned(),
        )
    );
    let active_runtime: (String, String) = connection
        .query_row(
            "SELECT id, state FROM runtime_instances
             WHERE id = ?1 AND removed_at IS NULL",
            [&first_active_runtime],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(active_runtime.1, "running");
}

#[test]
fn accepts_native_repository_paths_but_requires_textual_name_and_revision() {
    let database_path = temporary_database_path();
    let invalid_utf8 = OsString::from_vec(vec![0xff]);
    let native_path = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["app", "deploy-source", "missing-application"])
        .arg(&invalid_utf8)
        .args(["--revision", "main"])
        .output()
        .unwrap();
    let invalid_name = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["app", "deploy-source"])
        .arg(&invalid_utf8)
        .args(["repository", "--revision", "main"])
        .output()
        .unwrap();
    let invalid_revision = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args([
            "app",
            "deploy-source",
            "missing-application",
            "repository",
            "--revision",
        ])
        .arg(invalid_utf8)
        .output()
        .unwrap();
    let _ = fs::remove_file(&database_path);

    assert!(
        String::from_utf8_lossy(&native_path.stderr)
            .contains("application `missing-application` was not found")
    );
    for output in [invalid_name, invalid_revision] {
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
    }
}

#[test]
fn reports_desired_and_observed_state_after_deployment() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    let output = environment.run_lifecycle("status");

    assert_command_succeeded(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(
        lines[0],
        format!("Application: {}", environment.application_name)
    );
    assert_eq!(lines[1], "Desired state: Running");
    assert_eq!(lines[2], "Observed state: Running");
    assert!(lines[3].starts_with("Runtime: "));
    assert!(lines[4].starts_with("Container: "));
}

#[test]
fn stop_and_start_are_idempotent_and_persist_desired_and_observed_states() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    assert_command_succeeded(&environment.run_lifecycle("stop"));
    assert_command_succeeded(&environment.run_lifecycle("stop"));
    let connection = database::open(&environment.database_path).unwrap();
    let (desired, observed): (String, String) = current_runtime_states(&connection);
    drop(connection);
    assert_eq!(
        (desired, observed),
        ("stopped".to_owned(), "stopped".to_owned())
    );

    assert_command_succeeded(&environment.run_lifecycle("start"));
    assert_command_succeeded(&environment.run_lifecycle("start"));
    let connection = database::open(&environment.database_path).unwrap();
    let (desired, observed): (String, String) = current_runtime_states(&connection);
    assert_eq!(
        (desired, observed),
        ("running".to_owned(), "running".to_owned())
    );
}

#[test]
fn lifecycle_commands_fail_for_a_non_deployed_application_without_external_effects() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    for subcommand in ["status", "stop", "start"] {
        let output = environment.run_lifecycle(subcommand);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("is not deployed"),
            "unexpected stderr for `{subcommand}`: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(
        !environment.root.join("podman.log").exists(),
        "podman must not be invoked for a non-deployed application"
    );
}

#[test]
fn lifecycle_commands_report_an_unknown_application() {
    let database_path = temporary_database_path();

    for subcommand in ["status", "stop", "start"] {
        let output = run_pneuma(
            &database_path,
            &[
                OsStr::new("app"),
                OsStr::new(subcommand),
                OsStr::new("missing-application"),
            ],
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("application `missing-application` was not found")
        );
    }
    let _ = fs::remove_file(&database_path);
}

#[test]
fn a_removed_container_guides_a_new_deployment() {
    let status_environment = DeploymentEnvironment::new();
    assert_command_succeeded(&status_environment.import());
    status_environment.deploy_current_revision();
    fs::write(status_environment.root.join("podman-removed"), "removed").unwrap();

    let status = status_environment.run_lifecycle("status");
    assert!(!status.status.success());
    let status_stderr = String::from_utf8_lossy(&status.stderr);
    assert!(status_stderr.contains("is missing"));
    assert!(status_stderr.contains("pneuma app deploy"));

    let stop_environment = DeploymentEnvironment::new();
    assert_command_succeeded(&stop_environment.import());
    stop_environment.deploy_current_revision();
    fs::write(stop_environment.root.join("podman-removed"), "removed").unwrap();

    let stop = stop_environment.run_lifecycle("stop");
    assert!(!stop.status.success());
    let stop_stderr = String::from_utf8_lossy(&stop.stderr);
    assert!(stop_stderr.contains("is missing"));
    assert!(stop_stderr.contains("pneuma app deploy"));

    let connection = database::open(&stop_environment.database_path).unwrap();
    let observed: String = connection
        .query_row(
            "SELECT last_observed_state FROM runtime_instances WHERE state = 'running'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(observed, "missing");
}

#[test]
fn lists_deployments_for_a_deployed_application() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .args(["app", "deployments", &environment.application_name])
        .output()
        .unwrap();

    assert_command_succeeded(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[0],
        format!("Deployments for {}:", environment.application_name)
    );
    assert_eq!(lines[1], "DEPLOYMENT\tRELEASE\tSOURCE\tSTATUS");
    assert!(lines[2].contains("Succeeded"));
    assert!(lines[2].contains(&environment.commit_sha));
}

#[test]
fn lists_no_deployments_for_an_application_without_deployment_history() {
    let database_path = temporary_database_path();
    let repository_path = fixture_path("valid");
    assert_command_succeeded(&run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    ));

    let output = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("deployments"),
            OsStr::new("personal-site"),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "No deployments for personal-site\n"
    );
}

#[test]
fn deployments_command_fails_for_a_missing_application() {
    let database_path = temporary_database_path();

    let output = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("deployments"),
            OsStr::new("missing-application"),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("application `missing-application` was not found")
    );
}

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
    assert!(stderr.contains("Usage:"));
    assert!(stderr.contains("app visibility set"));
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
}

#[test]
fn deployments_source_is_dash_for_oci_releases() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let reference = format!("registry.example/team/service@sha256:{}", "a".repeat(64));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy_oci(&reference, port));
    server.join().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .args(["app", "deployments", &environment.application_name])
        .output()
        .unwrap();

    assert_command_succeeded(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1], "DEPLOYMENT\tRELEASE\tSOURCE\tSTATUS");
    assert!(lines[2].contains(&format!("sha256:{}", "a".repeat(64))));
    assert!(lines[2].contains("\t-\tSucceeded"));
}

fn run_visibility_command(environment: &DeploymentEnvironment, visibility: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pneuma"))
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

fn run_pneuma(database_path: &Path, arguments: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", database_path)
        .args(arguments)
        .output()
        .unwrap()
}

fn temporary_database_path() -> PathBuf {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!(
        "pneuma-cli-{}-{unique_suffix}.sqlite3",
        std::process::id()
    ))
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
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
    commit_sha: String,
}

#[derive(Clone, Copy, Debug)]
enum OciFailure {
    Pull,
    DigestMismatch,
}

impl DeploymentEnvironment {
    fn new() -> Self {
        Self::from_fixture("another", "another-site")
    }

    fn public() -> Self {
        Self::from_fixture("valid", "personal-site")
    }

    fn from_fixture(fixture: &str, application_name: &str) -> Self {
        let root = env::temp_dir().join(format!(
            "pneuma-cli-deploy-{}-{}",
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
        let commit_sha = git(&repository_path, &["rev-parse", "HEAD"])
            .trim()
            .to_owned();

        Self {
            root,
            repository_path,
            database_path,
            workspace_path,
            fake_bin,
            application_name: application_name.to_owned(),
            managed_caddy_directory,
            caddyfile_path,
            commit_sha,
        }
    }

    fn import(&self) -> Output {
        run_pneuma(
            &self.database_path,
            &[
                OsStr::new("app"),
                OsStr::new("import"),
                self.repository_path.as_os_str(),
            ],
        )
    }

    fn commit(&self, contents: &str) {
        fs::write(self.repository_path.join("site.txt"), contents).unwrap();
        git(&self.repository_path, &["add", "site.txt"]);
        git(
            &self.repository_path,
            &[
                "-c",
                "user.name=Pneuma Tests",
                "-c",
                "user.email=pneuma@example.invalid",
                "commit",
                "--quiet",
                "-m",
                contents,
            ],
        );
    }

    fn deploy(&self, port: u16, verbose: bool) -> Output {
        self.deploy_with_external_status(port, verbose, 200)
    }

    fn deploy_current_revision(&self) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || respond_once(&listener, 200));
        assert_command_succeeded(&self.deploy(port, false));
        server.join().unwrap();
    }

    fn run_lifecycle(&self, subcommand: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pneuma"))
            .env("PNEUMA_DATABASE_PATH", &self.database_path)
            .env("PNEUMA_WORKSPACE_PATH", &self.workspace_path)
            .env("PATH", executable_path(&self.fake_bin))
            .env("PNEUMA_FAKE_PORT", "30000")
            .env(
                "PNEUMA_FAKE_CONTAINER_STATE",
                self.root.join("container-state"),
            )
            .env("PNEUMA_FAKE_PODMAN_LOG", self.root.join("podman.log"))
            .env("PNEUMA_QUADLET_DIR", self.root.join("quadlets"))
            .env(
                "PNEUMA_FAKE_CONTAINER_STATE",
                self.root.join("container-state"),
            )
            .env(
                "PNEUMA_FAKE_PODMAN_REMOVED",
                self.root.join("podman-removed"),
            )
            .args(["app", subcommand, &self.application_name])
            .output()
            .unwrap()
    }

    fn deploy_with_external_status(
        &self,
        port: u16,
        verbose: bool,
        external_status: u16,
    ) -> Output {
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
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .env("PNEUMA_FAKE_CURL_STATUS", external_status.to_string());
        if verbose {
            command.arg("--verbose");
        }
        command
            .args(["app", "deploy-source", &self.application_name])
            .arg(&self.repository_path)
            .args(["--revision", "HEAD"])
            .output()
            .unwrap()
    }

    fn deploy_oci(&self, reference: &str, port: u16) -> Output {
        self.run_oci_deploy(reference, port, None)
    }

    fn deploy_oci_with_failure(&self, reference: &str, failure: OciFailure) -> Output {
        self.run_oci_deploy(reference, 30000, Some(failure))
    }

    fn run_oci_deploy(&self, reference: &str, port: u16, failure: Option<OciFailure>) -> Output {
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
            .env("PNEUMA_FAKE_PODMAN_LOG", self.root.join("podman.log"));
        match failure {
            Some(OciFailure::Pull) => {
                command.env(
                    "PNEUMA_FAKE_PODMAN_PULL_FAILURE",
                    self.root.join("pull-failure"),
                );
                fs::write(self.root.join("pull-failure"), "fail").unwrap();
            }
            Some(OciFailure::DigestMismatch) => {
                command.env(
                    "PNEUMA_FAKE_PODMAN_DIGEST",
                    format!("sha256:{}", "b".repeat(64)),
                );
            }
            None => {
                command.env(
                    "PNEUMA_FAKE_PODMAN_DIGEST",
                    format!("sha256:{}", "a".repeat(64)),
                );
            }
        }
        command
            .args([
                "app",
                "deploy",
                &self.application_name,
                "--image",
                reference,
            ])
            .output()
            .unwrap()
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
        if [ "$2" = "exists" ] && [ -f "${PNEUMA_FAKE_PODMAN_REMOVED:-}" ]; then
            exit 1
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
            "#!/bin/sh\nset -eu\ncase \"$1\" in validate) printf 'valid configuration\\n' ;; reload) printf 'reload complete\\n' ;; *) exit 1 ;; esac\n",
        ),
        (
            "curl",
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_CURL_LOG\"\nprintf '%s' \"${PNEUMA_FAKE_CURL_STATUS:-200}\"\n",
        ),
    ] {
        let executable = fake_bin.join(name);
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).unwrap();
    }
}

fn executable_path(fake_bin: &Path) -> OsString {
    let inherited = env::var_os("PATH").unwrap_or_default();
    env::join_paths(std::iter::once(fake_bin.to_path_buf()).chain(env::split_paths(&inherited)))
        .unwrap()
}

fn respond_once(listener: &TcpListener, status: u16) {
    let (mut stream, _) = listener.accept().unwrap();
    read_request(&mut stream);
    let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n");
    stream.write_all(response.as_bytes()).unwrap();
}

fn respond_unhealthy(listener: &TcpListener, attempts: usize) {
    for _ in 0..attempts {
        respond_once(listener, 500);
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

fn assert_identifier_line(line: &str, prefix: &str) {
    let identifier = line.strip_prefix(prefix).unwrap();
    assert_eq!(identifier.len(), 32);
    assert!(identifier.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

fn current_runtime_states(connection: &rusqlite::Connection) -> (String, String) {
    connection
        .query_row(
            "SELECT applications.desired_runtime_state, runtime_instances.last_observed_state
             FROM applications
              JOIN runtime_instances ON runtime_instances.deployment_id = applications.active_deployment_id
              WHERE runtime_instances.removed_at IS NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn assert_command_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
