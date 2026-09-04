use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::net::{Ipv4Addr, TcpListener};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use pneuma::adapters::database;

use crate::support::{
    DeploymentEnvironment, OciFailure, assert_command_succeeded, assert_identifier_line,
    create_repository_from_fixture, executable_path, respond_once, run_pneuma, run_pneuma_env,
    temporary_database_path, temporary_workspace_path, wait_for_child, wait_for_file,
};

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
    let digest = format!("sha256:{}", "a".repeat(64));
    assert_eq!(
        lines[1],
        format!("Image: {}@{digest}", environment.image_repository)
    );
    assert_identifier_line(lines[2], "Deployment: ");
    assert_identifier_line(lines[3], "Runtime: ");
    assert!(lines[4].starts_with("Container: pneuma-another-site-"));
    assert_eq!(lines[5], "Status: Succeeded");
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
fn deployment_continues_when_non_tty_stderr_rejects_progress_writes() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let read_only_null = fs::File::open("/dev/null").unwrap();
    let mut command = environment.deploy_command(port);
    command.stderr(Stdio::from(read_only_null));
    let output = command.output().unwrap();
    server.join().unwrap();

    assert_command_succeeded(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("Deployed another-site\n")
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
fn verbose_deploy_reports_lifecycle_steps_on_stderr() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.deploy(port, true);
    server.join().unwrap();

    assert_command_succeeded(&output);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("load application specification: started"));
    assert!(stderr.contains("create deployment: completed"));
    assert!(stderr.contains("health check and promotion: completed"));
}

#[test]
fn ci_dispatch_deploys_a_branch_with_the_existing_progress_contract() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.ci_dispatch(port);
    server.join().unwrap();

    assert_command_succeeded(&output);
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Deploying another-site...\n"
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("Deployed another-site\n")
    );
}

#[test]
fn deploy_writes_a_boot_enabled_quadlet_unit() {
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
    let release: (String, Option<String>) = connection
        .query_row(
            "SELECT r.image_reference, d.source_revision
             FROM releases r
             JOIN deployments d ON d.release_id = r.id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    // The canonical reference is the one persisted artifact identity; the
    // repository and digest are derived from it by parsing.
    assert_eq!(release, (reference.clone(), None));
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
    let state: (String, String, String) = connection
        .query_row(
            "SELECT exposures.materialization_state,
                     runtime_instances.state,
                    deployments.status
             FROM exposures
             JOIN runtime_instances ON runtime_instances.id = exposures.active_runtime_id
             JOIN deployments ON deployments.id = runtime_instances.deployment_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        state,
        (
            "active".to_owned(),
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
    let first_digest = format!("sha256:{}", "a".repeat(64));
    let first_reference = format!("{}@{first_digest}", environment.image_repository);
    let first_output = environment.deploy_oci(&first_reference, first_port);
    first_server.join().unwrap();
    assert_command_succeeded(&first_output);
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
    let _second_server = thread::spawn(move || {
        for _ in 0..10 {
            if let Ok((mut stream, _)) = second_listener.accept() {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        }
    });

    let digest = format!("sha256:{}", "b".repeat(64));
    let reference = format!("{}@{digest}", environment.image_repository);
    let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
    let output = command
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .env("PNEUMA_WORKSPACE_PATH", &environment.workspace_path)
        .env(
            "PNEUMA_CADDY_MANAGED_PATH",
            &environment.managed_caddy_directory,
        )
        .env("PNEUMA_CADDYFILE_PATH", &environment.caddyfile_path)
        .env("PNEUMA_QUADLET_DIR", environment.root.join("quadlets"))
        .env("PATH", executable_path(&environment.fake_bin))
        .env("PNEUMA_FAKE_PORT", second_port.to_string())
        .env(
            "PNEUMA_RUNTIME_PORT_RANGE",
            format!("{second_port}-{second_port}"),
        )
        .env(
            "PNEUMA_FAKE_PODMAN_COUNT",
            environment.root.join("podman-count"),
        )
        .env(
            "PNEUMA_FAKE_PODMAN_LOG",
            environment.root.join("podman.log"),
        )
        .env("PNEUMA_FAKE_CURL_LOG", environment.root.join("curl.log"))
        .env("PNEUMA_FAKE_CURL_STATUS", "503")
        .env("PNEUMA_FAKE_PODMAN_DIGEST", &digest)
        .args([
            "app",
            "deploy",
            &environment.application_name,
            "--image",
            &reference,
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("external_health_check_failed"),
        "expected an external health-check failure\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    assert_eq!(
        lines[1],
        "DEPLOYMENT\tTYPE\tRELEASE\tSOURCE\tSTATUS\tSTARTED\tFINISHED\tACTIVE\tFAILURE"
    );
    assert!(lines[2].contains("\tDeploy\t"));
    assert!(lines[2].contains("Succeeded"));
    assert!(lines[2].contains("\t-\t"));
}

#[test]
fn lists_no_deployments_for_an_application_without_deployment_history() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = create_repository_from_fixture(&workspace, "valid");
    let url = format!("file://{}", repository_path.display());
    assert_command_succeeded(&run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    ));

    let output = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("deployments"),
            OsStr::new("personal-site"),
        ],
    );
    let _ = fs::remove_dir_all(&workspace);
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
fn deploy_without_a_source_option_fails_before_database_or_external_work() {
    let environment = DeploymentEnvironment::new();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .env("PNEUMA_WORKSPACE_PATH", &environment.workspace_path)
        .env("PATH", executable_path(&environment.fake_bin))
        .env(
            "PNEUMA_FAKE_PODMAN_LOG",
            environment.root.join("podman.log"),
        )
        .args(["app", "deploy", &environment.application_name])
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&environment.root);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: either --image or --branch must be specified"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !environment.database_path.exists(),
        "argument normalization must fail before any database work"
    );
    assert!(
        !environment.root.join("podman.log").exists(),
        "argument normalization must fail before any external command"
    );
}

#[test]
fn deploy_accepts_branch_and_image_mutually_exclusively() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("{}@{digest}", environment.image_repository);

    let both = environment.run_oci_deploy(&reference, 30000, None, Some("staging"));
    assert!(!both.status.success());
    assert!(String::from_utf8_lossy(&both.stderr).contains("Usage:"));
    assert!(!environment.root.join("podman.log").exists());
}

#[test]
fn a_second_deploy_is_rejected_while_the_first_is_starting() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let marker = environment.root.join("systemctl-started");
    let release = environment.root.join("systemctl-release");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();

    let first = environment.spawn_gated_deploy(port, &marker, &release);
    wait_for_file(&marker, Duration::from_secs(2));

    let second = environment.deploy(port, false);
    assert!(!second.status.success());
    assert_eq!(second.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("already has an operation in progress"),
        "{stderr}"
    );
    assert!(!stderr.contains("database is locked"), "{stderr}");
    assert!(!stderr.contains("UNIQUE constraint"), "{stderr}");

    let server = thread::spawn(move || respond_once(&listener, 200));
    fs::write(&release, "release").unwrap();
    let first_output = wait_for_child(first, Duration::from_secs(5));
    server.join().unwrap();
    assert_command_succeeded(&first_output);

    let connection = database::open(&environment.database_path).unwrap();
    let deployment_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM deployments", [], |row| row.get(0))
        .unwrap();
    let runtime_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_instances WHERE state = 'running' AND removed_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deployment_count, 1);
    assert_eq!(runtime_count, 1);
}

#[test]
fn deploy_fails_with_exit_1_when_the_application_lock_cannot_be_opened() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let connection = database::open(&environment.database_path).unwrap();
    let application_id: String = connection
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    drop(connection);
    let lock_path = std::path::PathBuf::from(format!(
        "{}.{}.lock",
        environment.database_path.display(),
        application_id
    ));
    // A directory at the lock path makes every open attempt fail deterministically.
    fs::create_dir(&lock_path).unwrap();

    let output = environment.deploy(30000, false);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to open application lock"),
        "{stderr}"
    );
    assert!(
        stderr.contains(lock_path.to_string_lossy().as_ref()),
        "{stderr}"
    );
}

#[test]
fn deploy_fails_with_exit_5_when_systemd_cannot_start_the_container() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let failure_marker = environment.root.join("systemctl-start-failure");
    fs::write(&failure_marker, "fail").unwrap();

    let output = environment
        .deploy_command(30000)
        .env("PNEUMA_FAKE_SYSTEMCTL_START_FAILURE", &failure_marker)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runtime_start_failed"), "{stderr}");
}

#[test]
fn deploy_fails_with_exit_5_when_the_internal_health_check_fails() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    // No listener serves the candidate endpoint, so verification cannot confirm health.
    let output = environment.deploy(30000, false);

    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("health_check_failed"), "{stderr}");
}

#[test]
fn deploy_fails_with_exit_5_when_caddy_rejects_the_public_route() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    let failure_marker = environment.root.join("caddy-failure");
    fs::write(&failure_marker, "fail").unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment
        .deploy_command(port)
        .env("PNEUMA_FAKE_CADDY_FAILURE", &failure_marker)
        .output()
        .unwrap();
    server.join().unwrap();

    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("caddy_materialization_failed"), "{stderr}");
}

#[test]
fn deploy_fails_with_exit_5_when_the_external_health_check_fails() {
    let environment = DeploymentEnvironment::public();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.deploy_with_external_status(port, false, 500);
    server.join().unwrap();

    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("external_health_check_failed"), "{stderr}");
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
    assert_eq!(
        lines[1],
        "DEPLOYMENT\tTYPE\tRELEASE\tSOURCE\tSTATUS\tSTARTED\tFINISHED\tACTIVE\tFAILURE"
    );
    assert!(lines[2].contains("\tDeploy\t"));
    assert!(lines[2].contains(&format!("sha256:{}", "a".repeat(64))));
    assert!(lines[2].contains("\t-\tSucceeded"));
}

#[test]
fn rollback_without_previous_deployment_exits_with_code_four() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let output = run_pneuma_env(
        &environment.database_path,
        Some(&environment.workspace_path),
        &[
            OsStr::new("deployment"),
            OsStr::new("rollback"),
            OsStr::new(&environment.application_name),
        ],
    );

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no previous successful deployment"),
        "unexpected stderr: {stderr}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn tui_deploys_from_a_branch_form_and_restores_the_pseudo_terminal() {
    use std::fs::File;
    use std::io::Read;
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::time::Instant;

    fn duplicate(file: &File) -> File {
        let descriptor = unsafe { libc::dup(file.as_raw_fd()) };
        assert_ne!(descriptor, -1, "failed to duplicate pseudo-terminal");
        unsafe { File::from_raw_fd(descriptor) }
    }

    fn local_flags(file: &File) -> libc::tcflag_t {
        let mut attributes = MaybeUninit::<libc::termios>::uninit();
        let result = unsafe { libc::tcgetattr(file.as_raw_fd(), attributes.as_mut_ptr()) };
        assert_eq!(result, 0, "failed to inspect pseudo-terminal mode");
        unsafe { attributes.assume_init().c_lflag }
    }

    fn wait_for_succeeded_deployment(database_path: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let deployments = database::open(database_path).ok().and_then(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM deployments
                         WHERE type = 'deploy' AND status = 'succeeded'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .ok()
            });
            if deployments == Some(1) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the succeeded branch deployment"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    // Removes cursor-control sequences so words that were first drawn onto a
    // blank row appear contiguously in the remaining text stream.
    fn strip_escape_sequences(stream: &str) -> String {
        let mut text = String::new();
        let mut characters = stream.chars();
        while let Some(character) = characters.next() {
            if character != '\u{1b}' {
                text.push(character);
                continue;
            }
            if characters.next() == Some('[') {
                for escaped in characters.by_ref() {
                    if ('@'..='~').contains(&escaped) {
                        break;
                    }
                }
            }
        }
        text
    }

    // Replays the diff stream onto a virtual grid so the final screen state can
    // be asserted as plain text regardless of which cells each frame redraws.
    fn final_screen_text(stream: &str) -> String {
        const ROWS: usize = 40;
        const COLUMNS: usize = 120;
        let mut grid = vec![vec![' '; COLUMNS]; ROWS];
        let (mut row, mut column) = (0usize, 0usize);
        let mut characters = stream.chars();
        while let Some(character) = characters.next() {
            if character != '\u{1b}' {
                if character != '\n' && character != '\r' {
                    if row < ROWS && column < COLUMNS {
                        grid[row][column] = character;
                    }
                    column += 1;
                }
                continue;
            }
            if characters.next() != Some('[') {
                continue;
            }
            let mut parameters = String::new();
            let final_byte = loop {
                match characters.next() {
                    Some(c) if c.is_ascii_digit() || c == ';' || c == '?' => parameters.push(c),
                    Some(c) => break c,
                    None => panic!("unterminated escape sequence"),
                }
            };
            if final_byte == 'H' {
                let mut parts = parameters.split(';');
                let line = parts
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1);
                let next_column = parts
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1);
                row = line.saturating_sub(1);
                column = next_column.saturating_sub(1);
            }
        }
        grid.into_iter()
            .map(|line| line.into_iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let mut master = -1;
    let mut slave = -1;
    let window_size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &window_size,
        )
    };
    assert_eq!(opened, 0, "failed to open pseudo-terminal");

    let mut input = unsafe { File::from_raw_fd(master) };
    let terminal = unsafe { File::from_raw_fd(slave) };
    let original_flags = local_flags(&terminal);
    let output_reader = duplicate(&input);
    let collected = thread::spawn(move || {
        let mut reader = output_reader;
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            }
        }
        bytes
    });

    let digest = format!("sha256:{}", "a".repeat(64));
    let mut child = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .env("PNEUMA_WORKSPACE_PATH", &environment.workspace_path)
        .env(
            "PNEUMA_CADDY_MANAGED_PATH",
            &environment.managed_caddy_directory,
        )
        .env("PNEUMA_CADDYFILE_PATH", &environment.caddyfile_path)
        .env("PNEUMA_QUADLET_DIR", environment.root.join("quadlets"))
        .env("PATH", executable_path(&environment.fake_bin))
        .env("PNEUMA_FAKE_PORT", port.to_string())
        .env("PNEUMA_RUNTIME_PORT_RANGE", format!("{port}-{port}"))
        .env(
            "PNEUMA_FAKE_PODMAN_COUNT",
            environment.root.join("podman-count"),
        )
        .env(
            "PNEUMA_FAKE_PODMAN_LOG",
            environment.root.join("podman.log"),
        )
        .env("PNEUMA_FAKE_CURL_LOG", environment.root.join("curl.log"))
        .env("PNEUMA_FAKE_CURL_STATUS", "200")
        .env("PNEUMA_FAKE_PODMAN_DIGEST", digest)
        .env("PNEUMA_ASSERT_CLOSED_DATABASE", &environment.database_path)
        .args(["tui"])
        .stdin(Stdio::from(duplicate(&terminal)))
        .stdout(Stdio::from(duplicate(&terminal)))
        .stderr(Stdio::from(duplicate(&terminal)))
        .spawn()
        .unwrap();

    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while local_flags(&terminal) == original_flags {
        assert!(
            Instant::now() < ready_deadline,
            "TUI did not enter raw mode"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // Open the application details once the initial catalog has loaded.
    for _ in 0..10 {
        input.write_all(b"\r").unwrap();
        input.flush().unwrap();
        thread::sleep(Duration::from_millis(100));
    }
    thread::sleep(Duration::from_millis(300));

    // Open the deploy form and submit the current branch.
    input.write_all(b"d").unwrap();
    input.flush().unwrap();
    thread::sleep(Duration::from_millis(100));
    input.write_all(b"main").unwrap();
    input.flush().unwrap();
    thread::sleep(Duration::from_millis(100));
    input.write_all(b"\r").unwrap();
    input.flush().unwrap();

    wait_for_succeeded_deployment(&environment.database_path);
    // Let the deployment command finish and its typed result frame render
    // before quitting, so the settled screen carries the final outcome.
    thread::sleep(Duration::from_secs(2));

    input.write_all(b"q").unwrap();
    input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "TUI did not exit successfully: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "TUI did not exit after q");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(local_flags(&terminal), original_flags);
    drop(terminal);
    drop(child);
    let screen = String::from_utf8_lossy(&collected.join().unwrap()).to_string();
    // Semantic deployment events render with the shared CLI step vocabulary;
    // each step is first drawn onto a blank progress row, so its words survive
    // the diff stream contiguously after stripping cursor-control sequences.
    let text = strip_escape_sequences(&screen);
    assert!(
        text.contains("pullimage:started"),
        "expected semantic deployment progress on screen: {screen:?}"
    );
    // The typed final result stays visible in the detail view, so replay the
    // stream onto a grid and assert the settled screen contents.
    let settled = final_screen_text(&screen);
    assert!(
        settled.contains("Deployed another-site: deployment"),
        "expected the typed deploy result on the final screen: {settled:?}"
    );
    assert!(
        settled.contains("promoted"),
        "expected the promoted artifact summary on the final screen: {settled:?}"
    );
    server.join().unwrap();

    let connection = database::open(&environment.database_path).unwrap();
    let source_revision: Option<String> = connection
        .query_row(
            "SELECT source_revision FROM deployments WHERE type = 'deploy' AND status = 'succeeded'",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap();
    assert!(
        source_revision.is_some(),
        "branch deploy must record its revision"
    );
}
