use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::net::{Ipv4Addr, TcpListener};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use pneuma::adapters::database;

use rusqlite::OptionalExtension;

use crate::support::{
    DeploymentEnvironment, assert_command_succeeded, executable_path, respond_once, run_pneuma,
    temporary_database_path,
};

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

    let stopped = environment.run_lifecycle("stop");
    assert_command_succeeded(&stopped);
    assert_eq!(
        String::from_utf8_lossy(&stopped.stdout),
        format!("Stopped {}\n", environment.application_name)
            + "Desired state: Stopped\nObserved state: Stopped\n"
    );
    assert_command_succeeded(&environment.run_lifecycle("stop"));
    let connection = database::open(&environment.database_path).unwrap();
    let (desired, observed): (String, String) = current_runtime_states(&connection);
    drop(connection);
    assert_eq!(
        (desired, observed),
        ("stopped".to_owned(), "stopped".to_owned())
    );

    let started = environment.run_lifecycle("start");
    assert_command_succeeded(&started);
    assert_eq!(
        String::from_utf8_lossy(&started.stdout),
        format!("Started {}\n", environment.application_name)
            + "Desired state: Running\nObserved state: Running\n"
    );
    assert_command_succeeded(&environment.run_lifecycle("start"));
    let connection = database::open(&environment.database_path).unwrap();
    let (desired, observed): (String, String) = current_runtime_states(&connection);
    assert_eq!(
        (desired, observed),
        ("running".to_owned(), "running".to_owned())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn tui_starts_an_application_after_stopping_it_without_leaving_details() {
    use std::fs::File;
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd};

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

    fn wait_for_desired_state(database_path: &std::path::Path, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = database::open(database_path).ok().and_then(|connection| {
                connection
                    .query_row(
                        "SELECT desired_runtime_state FROM applications LIMIT 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
            });
            if state.as_deref() == Some(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for desired state `{expected}`, got {state:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    let mut master = -1;
    let mut slave = -1;
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "failed to open pseudo-terminal");

    let mut input = unsafe { File::from_raw_fd(master) };
    let terminal = unsafe { File::from_raw_fd(slave) };
    let original_flags = local_flags(&terminal);
    let mut child = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .env("PNEUMA_WORKSPACE_PATH", &environment.workspace_path)
        .env("PNEUMA_QUADLET_DIR", environment.root.join("quadlets"))
        .env("PATH", executable_path(&environment.fake_bin))
        .env("PNEUMA_FAKE_PORT", "30000")
        .env(
            "PNEUMA_FAKE_CONTAINER_STATE",
            environment.root.join("container-state"),
        )
        .env(
            "PNEUMA_FAKE_PODMAN_LOG",
            environment.root.join("podman.log"),
        )
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

    for _ in 0..10 {
        input.write_all(b"\r").unwrap();
        input.flush().unwrap();
        thread::sleep(Duration::from_millis(100));
    }
    input.write_all(b"x").unwrap();
    input.flush().unwrap();
    thread::sleep(Duration::from_millis(50));
    input.write_all(b"y").unwrap();
    input.flush().unwrap();
    wait_for_desired_state(&environment.database_path, "stopped");

    input.write_all(b"s").unwrap();
    input.flush().unwrap();
    thread::sleep(Duration::from_millis(50));
    input.write_all(b"y").unwrap();
    input.flush().unwrap();
    wait_for_desired_state(&environment.database_path, "running");

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
fn lifecycle_ignores_a_runtime_from_a_non_succeeded_deployment() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    let connection = database::open(&environment.database_path).unwrap();
    connection
        .execute(
            "UPDATE deployments SET status = 'failed', finished_at = CURRENT_TIMESTAMP,
                failure_code = 'runtime_start_failed', failure_stage = 'starting',
                failure_message = 'test'",
            [],
        )
        .unwrap();
    drop(connection);

    let status = environment.run_lifecycle("status");
    assert!(!status.status.success());
    assert!(
        String::from_utf8_lossy(&status.stderr).contains("is not deployed"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

#[test]
fn failed_start_keeps_the_requested_desired_state() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();
    assert_command_succeeded(&environment.run_lifecycle("stop"));
    fs::write(environment.root.join("systemctl-start-failure"), "fail").unwrap();

    let start = environment.run_lifecycle("start");
    assert!(!start.status.success());

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

    assert_command_succeeded(&stop_environment.run_lifecycle("stop"));

    let connection = database::open(&stop_environment.database_path).unwrap();
    let (observed, removed_at): (String, Option<String>) = connection
        .query_row(
            "SELECT last_observed_state, removed_at FROM runtime_instances WHERE removed_at IS NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(connection);
    assert_eq!(observed, "missing");
    assert!(
        removed_at.is_none(),
        "removed_at must remain NULL after stop with missing container"
    );
}

#[test]
fn stop_and_start_cycle_after_container_removal_by_quadlet() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    fs::write(environment.root.join("podman-removed"), "removed").unwrap();

    assert_command_succeeded(&environment.run_lifecycle("stop"));
    assert_command_succeeded(&environment.run_lifecycle("stop"));

    let connection = database::open(&environment.database_path).unwrap();
    let (desired, observed, removed_at): (String, String, Option<String>) = connection
        .query_row(
            "SELECT a.desired_runtime_state, ri.last_observed_state, ri.removed_at
             FROM applications a
             JOIN runtime_instances ri ON ri.deployment_id = a.active_deployment_id
             WHERE a.id = (SELECT id FROM applications LIMIT 1)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    drop(connection);
    assert_eq!(desired, "stopped");
    assert_eq!(observed, "missing");
    assert!(
        removed_at.is_none(),
        "removed_at must remain NULL after stop with missing container"
    );

    fs::remove_file(environment.root.join("podman-removed")).unwrap();

    assert_command_succeeded(&environment.run_lifecycle("start"));
    assert_command_succeeded(&environment.run_lifecycle("start"));

    let connection = database::open(&environment.database_path).unwrap();
    let (desired, observed): (String, String) = current_runtime_states(&connection);
    drop(connection);
    assert_eq!(
        (desired, observed),
        ("running".to_owned(), "running".to_owned())
    );

    assert_command_succeeded(&environment.run_lifecycle("status"));
}

#[test]
fn status_preserves_missing_after_stop_when_container_was_removed() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    fs::write(environment.root.join("podman-removed"), "removed").unwrap();

    assert_command_succeeded(&environment.run_lifecycle("stop"));

    let status = environment.run_lifecycle("status");
    assert_command_succeeded(&status);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("Desired state: Stopped"), "{stdout}");
    assert!(stdout.contains("Observed state: Missing"), "{stdout}");

    let connection = database::open(&environment.database_path).unwrap();
    let removed_at: Option<String> = connection
        .query_row(
            "SELECT removed_at FROM runtime_instances WHERE removed_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    drop(connection);
    assert!(
        removed_at.is_none(),
        "removed_at must remain NULL after status when stopped with missing container"
    );
}

#[test]
fn starts_a_verified_oci_image_after_its_container_is_removed() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/team/service@{digest}");

    let first_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let first_port = first_listener.local_addr().unwrap().port();
    let first_server = thread::spawn(move || respond_once(&first_listener, 200));
    assert_command_succeeded(&environment.deploy_oci(&reference, first_port));
    first_server.join().unwrap();

    fs::write(environment.root.join("podman-removed"), "removed").unwrap();
    let status = environment.run_lifecycle("status");
    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("is missing"));

    assert_command_succeeded(&environment.run_lifecycle("start"));
    let connection = database::open(&environment.database_path).unwrap();
    let deployment_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM deployments", [], |row| row.get(0))
        .unwrap();
    assert_eq!(deployment_count, 1);
}

#[test]
fn status_does_not_reconcile_a_container_recreated_under_the_stable_name() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let reference = format!("registry.example/team/service@sha256:{}", "a".repeat(64));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    assert_command_succeeded(&environment.deploy_oci(&reference, port));
    server.join().unwrap();

    let connection = database::open(&environment.database_path).unwrap();
    let recorded: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let replacement = "c".repeat(64);
    environment.stale_container_id = Some(recorded.clone());
    environment.replacement_container_id = Some(replacement.clone());

    let status = environment.run_lifecycle("status");
    assert!(!status.status.success());
    let connection = database::open(&environment.database_path).unwrap();
    let persisted: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted, recorded);
}

#[test]
fn status_does_not_attempt_external_id_cas() {
    let mut environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    let connection = database::open(&environment.database_path).unwrap();
    let recorded: String = connection
        .query_row(
            "SELECT external_runtime_id FROM runtime_instances",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_external_runtime_id_reconciliation
             BEFORE UPDATE OF external_runtime_id ON runtime_instances
             BEGIN
                 SELECT RAISE(IGNORE);
             END",
        )
        .unwrap();
    drop(connection);

    environment.stale_container_id = Some(recorded);
    environment.replacement_container_id = Some("c".repeat(64));
    let status = environment.run_lifecycle("status");

    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("is missing"));
}

#[test]
fn status_preserves_the_expected_endpoint_and_updates_observation_timestamp() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    environment.deploy_current_revision();

    let connection = database::open(&environment.database_path).unwrap();
    connection
        .execute(
            "UPDATE runtime_instances
             SET host_port = 30001, last_observed_at = '2000-01-01 00:00:00'",
            [],
        )
        .unwrap();
    drop(connection);

    assert_command_succeeded(&environment.run_lifecycle("status"));

    let connection = database::open(&environment.database_path).unwrap();
    let (host_port, observed_at): (u16, String) = connection
        .query_row(
            "SELECT host_port, last_observed_at FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(host_port, 30001);
    assert_ne!(observed_at, "2000-01-01 00:00:00");
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
