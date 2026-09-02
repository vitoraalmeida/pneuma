use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pneuma::adapters::application_lock::ApplicationLock;
use pneuma::adapters::database::{self, DatabaseLock, LockMode};

use rusqlite::OptionalExtension;

#[test]
fn imports_and_lists_an_application_idempotently() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = create_repository_from_fixture(&workspace, "valid");
    let url = format!("file://{}", repository_path.display());

    let first_import = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );
    let second_import = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );
    let list = run_pneuma(&database_path, &[OsStr::new("app"), OsStr::new("list")]);
    let _ = fs::remove_dir_all(&workspace);
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

fn temporary_workspace_path() -> PathBuf {
    env::temp_dir().join(format!(
        "pneuma-cli-workspace-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
}

fn create_repository_from_fixture(workspace: &Path, fixture: &str) -> PathBuf {
    let repository_path = workspace.join("remote");
    fs::create_dir_all(&repository_path).unwrap();
    fs::copy(
        fixture_path(fixture).join("pneuma.toml"),
        repository_path.join("pneuma.toml"),
    )
    .unwrap();
    initialize_repository(&repository_path);
    repository_path
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run_pneuma_env(
    database_path: &Path,
    workspace_path: Option<&Path>,
    arguments: &[&OsStr],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
    command
        .env("PNEUMA_DATABASE_PATH", database_path)
        .args(arguments);
    if let Some(workspace_path) = workspace_path {
        command.env("PNEUMA_WORKSPACE_PATH", workspace_path);
    }
    command.output().unwrap()
}

fn run_pneuma(database_path: &Path, arguments: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", database_path)
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn reports_manifest_errors_and_returns_failure() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = workspace.join("remote");
    fs::create_dir_all(&repository_path).unwrap();
    fs::write(repository_path.join("README.md"), "missing manifest\n").unwrap();
    initialize_repository(&repository_path);
    let url = format!("file://{}", repository_path.display());

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("pneuma.toml"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_a_local_import_path_without_creating_an_application() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = fixture_path("valid");

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("application imports require a Git URL; local paths are not supported")
    );
    let connection = database::open(&database_path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert!(!workspace.exists());
    let _ = fs::remove_file(&database_path);
}

#[test]
fn cleans_the_temporary_checkout_after_a_clone_failure() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let url = format!("file://{}/missing", workspace.display());

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );

    assert!(!output.status.success());
    let imports = workspace.join("imports");
    assert!(imports.exists());
    assert!(fs::read_dir(&imports).unwrap().next().is_none());
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);
}

#[test]
fn imports_from_a_remote_git_url_with_a_manifest_path() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let remote = workspace.join("remote");
    fs::create_dir_all(remote.join("deploy/staging")).unwrap();
    fs::copy(
        fixture_path("valid/deploy/staging/pneuma.toml"),
        remote.join("deploy/staging/pneuma.toml"),
    )
    .unwrap();
    initialize_repository(&remote);
    let url = format!("file://{}", remote.display());

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            OsStr::new(&url),
            OsStr::new("--manifest"),
            OsStr::new("deploy/staging/pneuma.toml"),
        ],
    );

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Imported personal-site\nStatus: Registered\nDeployment: Not deployed\n"
    );
    let connection = database::open(&database_path).unwrap();
    let (repository_url, manifest_path): (String, String) = connection
        .query_row(
            "SELECT repository_url, manifest_path FROM applications",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(repository_url, url);
    assert_eq!(manifest_path, "deploy/staging/pneuma.toml");
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);
}

#[test]
fn remote_import_is_idempotent() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let remote = workspace.join("remote");
    fs::create_dir_all(&remote).unwrap();
    fs::copy(
        fixture_path("valid/pneuma.toml"),
        remote.join("pneuma.toml"),
    )
    .unwrap();
    initialize_repository(&remote);
    let url = format!("file://{}", remote.display());
    let arguments = &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)];

    let first = run_pneuma_env(&database_path, Some(&workspace), arguments);
    let second = run_pneuma_env(&database_path, Some(&workspace), arguments);

    assert!(first.status.success());
    assert!(second.status.success());
    let connection = database::open(&database_path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let exposure_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM exposures", [], |row| row.get(0))
        .unwrap();
    assert_eq!(exposure_count, 1);
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);
}

#[test]
fn reports_an_unusable_database_location_and_returns_failure() {
    let database_path = temporary_database_path()
        .join("missing")
        .join("pneuma.sqlite3");

    let output = run_pneuma(&database_path, &[OsStr::new("app"), OsStr::new("list")]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The shared database lock is acquired before the database itself is opened,
    // so an unusable location surfaces at the lock boundary with the path named.
    assert!(stderr.contains("failed to acquire the database-wide lock"));
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized subcommand"));
    assert!(stderr.contains("Usage"));
}

#[test]
fn doctor_reports_captured_diagnostics_for_a_failed_command() {
    let database_path = temporary_database_path();
    let fake_bin = temporary_workspace_path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(
        fake_bin.join("git"),
        "#!/bin/sh\necho 'fatal: bad object' >&2\nexit 128\n",
    )
    .unwrap();
    make_executable(&fake_bin.join("git"));

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .env("PATH", executable_path(&fake_bin))
        .args([OsStr::new("doctor")])
        .output()
        .unwrap();
    let _ = fs::remove_file(&database_path);
    let _ = fs::remove_dir_all(fake_bin.parent().unwrap());

    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("✗ Git: command failed (fatal: bad object)"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("one or more diagnostic checks failed"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn doctor_keeps_the_generic_line_when_a_failed_command_has_no_detail() {
    let database_path = temporary_database_path();
    let fake_bin = temporary_workspace_path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(fake_bin.join("git"), "#!/bin/sh\nexit 1\n").unwrap();
    make_executable(&fake_bin.join("git"));

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .env("PATH", executable_path(&fake_bin))
        .args([OsStr::new("doctor")])
        .output()
        .unwrap();
    let _ = fs::remove_file(&database_path);
    let _ = fs::remove_dir_all(fake_bin.parent().unwrap());

    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("✗ Git: command failed\n"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        !stdout.contains("✗ Git: command failed ("),
        "unexpected stdout: {stdout}"
    );
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
fn reimport_reports_the_real_state_of_a_deployed_application() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    let deploy = environment.deploy(port, false);
    server.join().unwrap();
    assert_command_succeeded(&deploy);
    let deployment_id = String::from_utf8(deploy.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Deployment: "))
        .unwrap()
        .to_owned();

    let reimport = environment.import();
    assert_command_succeeded(&reimport);
    let stdout = String::from_utf8(reimport.stdout).unwrap();
    assert!(
        stdout.contains(&format!("Deployment: {deployment_id}")),
        "unexpected stdout: {stdout}"
    );
    assert!(
        !stdout.contains("Not deployed"),
        "unexpected stdout: {stdout}"
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
    assert_eq!(
        String::from_utf8(backup_output.stdout).unwrap(),
        format!("Database backup: {}\n", backup.display())
    );
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
    assert!(
        String::from_utf8(restore_output.stdout)
            .unwrap()
            .starts_with(&format!(
                "Database restored from {}\nPre-restore backup: ",
                backup.display()
            ))
    );
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
fn restore_rejects_an_incompatible_backup_without_replacing_the_live_database() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let backup = environment.root.join("old-generation-backup.sqlite3");
    let connection = rusqlite::Connection::open(&backup).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             INSERT INTO schema_migrations (version) VALUES (14);",
        )
        .unwrap();
    drop(connection);

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .args(["database", "restore"])
        .arg(&backup)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("incompatible schema"),
        "unexpected stderr: {stderr}"
    );
    let connection = database::open(&environment.database_path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "the live catalog must stay untouched");
    assert!(
        !fs::read_dir(&environment.root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("pre-restore")
        }),
        "no pre-restore snapshot may be created for a rejected backup"
    );
}

#[test]
fn restore_conflicts_while_normal_access_holds_the_database_lock() {
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
    let _shared = DatabaseLock::try_acquire(&environment.database_path, LockMode::Shared)
        .unwrap()
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &environment.database_path)
        .args(["database", "restore"])
        .arg(&backup)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("another Pneuma command is using the database"),
        "unexpected stderr: {stderr}"
    );
    let connection = database::open(&environment.database_path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "a busy restore must not replace the database");
}

#[test]
fn normal_commands_conflict_while_restore_holds_the_exclusive_lock() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let _exclusive = DatabaseLock::try_acquire(&environment.database_path, LockMode::Exclusive)
        .unwrap()
        .unwrap();

    let output = run_pneuma(
        &environment.database_path,
        &[OsStr::new("app"), OsStr::new("list")],
    );

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("another Pneuma command is using the database"),
        "unexpected stderr: {stderr}"
    );
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
        .env("PNEUMA_FAKE_CURL_STATUS", curl_status.to_string())
        .env("PNEUMA_ASSERT_CLOSED_DATABASE", &environment.database_path)
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
    assert!(!public.status.success());
    assert!(
        String::from_utf8_lossy(&public.stderr).contains("requires a domain"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&public.stderr)
    );

    assert_exposure_state(&environment, "internal", "not_materialized", None);
    assert!(!environment.managed_caddy_directory.exists());
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
fn usage_errors_exit_with_code_two_and_keep_the_error_message() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());

    let output = environment.deploy_oci("not-a-digest", 30000);

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: image reference `not-a-digest`"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn unknown_applications_exit_with_code_three_and_name_the_application() {
    let database_path = temporary_database_path();

    let output = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("status"),
            OsStr::new("missing"),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: application `missing` was not found"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn external_failures_exit_with_code_five_and_report_the_integration() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let reference = format!("registry.example/team/service@sha256:{}", "a".repeat(64));

    let output = environment.deploy_oci_with_failure(&reference, OciFailure::Pull);

    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: failed to pull OCI image"),
        "unexpected stderr: {stderr}"
    );
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

#[test]
fn systems_round_trip_through_the_cli_with_unchanged_output() {
    let database_path = temporary_database_path();

    let create = run_pneuma(
        &database_path,
        &[
            OsStr::new("system"),
            OsStr::new("create"),
            OsStr::new("platform"),
            OsStr::new("--description"),
            OsStr::new("Team platform"),
        ],
    );
    let list = run_pneuma(&database_path, &[OsStr::new("system"), OsStr::new("list")]);
    let show = run_pneuma(
        &database_path,
        &[
            OsStr::new("system"),
            OsStr::new("show"),
            OsStr::new("platform"),
        ],
    );
    let missing = run_pneuma(
        &database_path,
        &[
            OsStr::new("system"),
            OsStr::new("show"),
            OsStr::new("missing"),
        ],
    );
    let invalid = run_pneuma(
        &database_path,
        &[
            OsStr::new("system"),
            OsStr::new("create"),
            OsStr::new("Not Valid"),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert_command_succeeded(&create);
    assert_eq!(
        String::from_utf8_lossy(&create.stdout),
        "Created platform\n"
    );
    assert_command_succeeded(&list);
    assert_eq!(String::from_utf8_lossy(&list.stdout), "platform\n");
    assert_command_succeeded(&show);
    assert_eq!(
        String::from_utf8_lossy(&show.stdout),
        "System: platform\nDescription: Team platform\nApplications: (none)\n"
    );
    assert_eq!(missing.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(
        stderr.contains("error: system `missing` was not found"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(invalid.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(
        stderr.contains("error: invalid system name `Not Valid`"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn fake_external_commands_fail_when_the_database_has_an_open_write_transaction() {
    let environment = DeploymentEnvironment::new();
    let journal = environment.database_path.with_file_name(format!(
        "{}-journal",
        environment
            .database_path
            .file_name()
            .unwrap()
            .to_string_lossy()
    ));
    fs::write(&journal, "").unwrap();

    for name in ["podman", "systemctl", "caddy", "curl"] {
        let output = Command::new(environment.fake_bin.join(name))
            .env("PNEUMA_ASSERT_CLOSED_DATABASE", &environment.database_path)
            .arg("any")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(90),
            "{name} did not enforce the closed-database guard\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("sqlite write transaction was open"),
            "{name} missing guard diagnostic: {stderr}"
        );
    }
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
    stale_container_id: Option<String>,
    replacement_container_id: Option<String>,
    replacement_application_label: Option<String>,
    reconciliation_port: Option<u16>,
    reconciliation_curl_status: Option<u16>,
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
            stale_container_id: None,
            replacement_container_id: None,
            replacement_application_label: None,
            reconciliation_port: None,
            reconciliation_curl_status: None,
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
        let mut command = self.deploy_command(port);
        command.env("PNEUMA_FAKE_CURL_STATUS", external_status.to_string());
        if verbose {
            command.arg("--verbose");
        }
        command.output().unwrap()
    }

    fn deploy_command(&self, port: u16) -> Command {
        let digest = format!("sha256:{}", "a".repeat(64));
        let reference = format!("{}@{digest}", self.image_repository);
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
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .env("PNEUMA_FAKE_CURL_STATUS", "200")
            .env("PNEUMA_FAKE_PODMAN_DIGEST", digest)
            .env("PNEUMA_ASSERT_CLOSED_DATABASE", &self.database_path)
            .args([
                "app",
                "deploy",
                &self.application_name,
                "--image",
                &reference,
            ]);
        command
    }

    fn ci_dispatch(&self, port: u16) -> Output {
        let digest = format!("sha256:{}", "a".repeat(64));
        Command::new(env!("CARGO_BIN_EXE_pneuma"))
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
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .env("PNEUMA_FAKE_CURL_STATUS", "200")
            .env("PNEUMA_FAKE_PODMAN_DIGEST", digest)
            .env("PNEUMA_ASSERT_CLOSED_DATABASE", &self.database_path)
            .env(
                "SSH_ORIGINAL_COMMAND",
                format!("deploy {} main", self.application_name),
            )
            .args(["ci", "dispatch"])
            .output()
            .unwrap()
    }

    fn deploy_current_revision(&self) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || respond_once(&listener, 200));
        assert_command_succeeded(&self.deploy(port, false));
        server.join().unwrap();
    }

    fn spawn_gated_deploy(&self, port: u16, marker: &Path, release: &Path) -> Child {
        let mut command = self.deploy_command(port);
        command
            .env("PNEUMA_FAKE_SYSTEMCTL_START_MARKER", marker)
            .env("PNEUMA_FAKE_SYSTEMCTL_START_RELEASE", release)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().unwrap()
    }

    fn deploy_oci(&self, reference: &str, port: u16) -> Output {
        self.run_oci_deploy(reference, port, None, None)
    }

    fn run_oci_deploy(
        &self,
        reference: &str,
        port: u16,
        failure: Option<OciFailure>,
        branch: Option<&str>,
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
            .env("PNEUMA_FAKE_PODMAN_LOG", self.root.join("podman.log"))
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .env("PNEUMA_FAKE_CURL_STATUS", "200")
            .env("PNEUMA_ASSERT_CLOSED_DATABASE", &self.database_path);
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
        command.args([
            "app",
            "deploy",
            &self.application_name,
            "--image",
            reference,
        ]);
        if let Some(branch) = branch {
            command.args(["--branch", branch]);
        }
        command.output().unwrap()
    }

    fn deploy_oci_with_failure(&self, reference: &str, failure: OciFailure) -> Output {
        self.run_oci_deploy(reference, 30000, Some(failure), None)
    }

    fn import(&self) -> Output {
        let repository_url = format!("file://{}", self.repository_path.display());
        run_pneuma_env(
            &self.database_path,
            Some(&self.workspace_path),
            &[
                OsStr::new("app"),
                OsStr::new("import"),
                OsStr::new(&repository_url),
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

    fn run_lifecycle(&self, subcommand: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
        command
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
            .env(
                "PNEUMA_FAKE_SYSTEMCTL_START_FAILURE",
                self.root.join("systemctl-start-failure"),
            )
            // The rollback journal exists exactly while a write transaction is
            // open, so fake external commands prove INV-WF-002 while they run.
            .env("PNEUMA_ASSERT_CLOSED_DATABASE", &self.database_path);
        if let Some(stale) = &self.stale_container_id {
            command.env("PNEUMA_FAKE_PODMAN_STALE_ID", stale);
        }
        if let Some(replacement) = &self.replacement_container_id {
            command.env("PNEUMA_FAKE_PODMAN_ID", replacement);
        }
        command
            .args(["app", subcommand, &self.application_name])
            .output()
            .unwrap()
    }

    fn run_reconcile(&self) -> Output {
        let digest = format!("sha256:{}", "a".repeat(64));
        let image_reference = format!("{}@{digest}", self.image_repository);
        let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
        command
            .env("PNEUMA_DATABASE_PATH", &self.database_path)
            .env("PNEUMA_WORKSPACE_PATH", &self.workspace_path)
            .env("PNEUMA_CADDY_MANAGED_PATH", &self.managed_caddy_directory)
            .env("PNEUMA_CADDYFILE_PATH", &self.caddyfile_path)
            .env("PNEUMA_QUADLET_DIR", self.root.join("quadlets"))
            .env("PATH", executable_path(&self.fake_bin))
            .env(
                "PNEUMA_FAKE_PORT",
                self.reconciliation_port.unwrap_or(30000).to_string(),
            )
            .env("PNEUMA_FAKE_PODMAN_LOG", self.root.join("podman.log"))
            .env(
                "PNEUMA_FAKE_CONTAINER_STATE",
                self.root.join("container-state"),
            )
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .env(
                "PNEUMA_FAKE_CURL_STATUS",
                self.reconciliation_curl_status.unwrap_or(200).to_string(),
            )
            .env("PNEUMA_FAKE_PODMAN_IMAGE", image_reference)
            .env(
                "PNEUMA_FAKE_APPLICATION_LABEL",
                self.replacement_application_label
                    .as_deref()
                    .unwrap_or(&self.application_name),
            )
            .env("PNEUMA_FAKE_IMAGE_DIGEST_LABEL", digest)
            .env(
                "PNEUMA_FAKE_PODMAN_REMOVED",
                self.root.join("podman-removed"),
            )
            .env("PNEUMA_ASSERT_CLOSED_DATABASE", &self.database_path)
            .args(["reconcile", &self.application_name]);
        if let Some(stale) = &self.stale_container_id {
            command.env("PNEUMA_FAKE_PODMAN_STALE_ID", stale);
        }
        if let Some(replacement) = &self.replacement_container_id {
            command.env("PNEUMA_FAKE_PODMAN_ID", replacement);
        }
        command.output().unwrap()
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

fn respond_once(listener: &TcpListener, status: u16) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                read_request(&mut stream);
                let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n");
                stream.write_all(response.as_bytes()).unwrap();
                return;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    panic!("health server timed out waiting for a request");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("health server failed to accept a request: {error}"),
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

fn install_fake_podman(fake_bin: &Path) {
    let podman = fake_bin.join("podman");
    fs::write(
        &podman,
        r#"#!/bin/sh
set -eu

if [ -n "${PNEUMA_ASSERT_CLOSED_DATABASE:-}" ] && [ -f "${PNEUMA_ASSERT_CLOSED_DATABASE}-journal" ]; then
    printf 'sqlite write transaction was open during a podman effect\n' >&2
    exit 90
fi

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
        removed_ids="${PNEUMA_FAKE_PODMAN_REMOVED_IDS:-${PNEUMA_FAKE_PODMAN_LOG:-}.removed}"
        if [ "$2" = "exists" ]; then
            if [ -f "${PNEUMA_FAKE_PODMAN_REMOVED:-}" ]; then
                exit 1
            fi
            if [ -n "${PNEUMA_FAKE_PODMAN_STALE_ID:-}" ] && [ "$3" = "$PNEUMA_FAKE_PODMAN_STALE_ID" ]; then
                exit 1
            fi
            if [ -f "$removed_ids" ] && grep -qxF "$3" "$removed_ids"; then
                exit 1
            fi
        elif [ "$2" = "rm" ]; then
            printf '%s\n' "$4" >> "$removed_ids"
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
        if [ -f "${PNEUMA_FAKE_PODMAN_REMOVED:-}" ]; then
            exit 1
        fi
        if [ "$2" = "--format" ] && [ "$3" = "{{.Id}}" ]; then
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
        elif printf '%s' "$3" | grep -q '.Config.Image'; then
            container_id="${PNEUMA_FAKE_PODMAN_ID:-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}"
            printf '%s\t/%s\t%s\t%s\t%s\n' "$container_id" "$4" "$PNEUMA_FAKE_PODMAN_IMAGE" "$PNEUMA_FAKE_APPLICATION_LABEL" "$PNEUMA_FAKE_IMAGE_DIGEST_LABEL"
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
if [ -n "${PNEUMA_ASSERT_CLOSED_DATABASE:-}" ] && [ -f "${PNEUMA_ASSERT_CLOSED_DATABASE}-journal" ]; then
    printf 'sqlite write transaction was open during a systemctl effect\n' >&2
    exit 90
fi
if [ "$1" = "--user" ]; then
    shift
fi
case "$1" in
    is-active)
        printf 'inactive\n'
        exit 3
        ;;
    daemon-reload|start|stop|enable|disable)
        if [ "$1" = "start" ] && [ -f "${PNEUMA_FAKE_SYSTEMCTL_START_FAILURE:-}" ]; then
            printf 'start failed\n' >&2
            exit 1
        fi
        if [ "$1" = "start" ] && [ -n "${PNEUMA_FAKE_SYSTEMCTL_START_MARKER:-}" ]; then
            : > "$PNEUMA_FAKE_SYSTEMCTL_START_MARKER"
            while [ ! -f "${PNEUMA_FAKE_SYSTEMCTL_START_RELEASE:-}" ]; do
                sleep 0.01
            done
        fi
        if [ "$1" = "start" ] && [ -n "${PNEUMA_FAKE_CONTAINER_STATE:-}" ]; then
            printf 'running\n' > "$PNEUMA_FAKE_CONTAINER_STATE"
            rm -f "${PNEUMA_FAKE_PODMAN_REMOVED:-}"
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
            "#!/bin/sh\nset -eu\nif [ -n \"${PNEUMA_ASSERT_CLOSED_DATABASE:-}\" ] && [ -f \"${PNEUMA_ASSERT_CLOSED_DATABASE}-journal\" ]; then\n    printf 'sqlite write transaction was open during a caddy effect\\n' >&2\n    exit 90\nfi\ncase \"$1\" in validate) printf 'valid configuration\\n' ;; reload) printf 'reload complete\\n' ;; *) exit 1 ;; esac\n",
        ),
        (
            "curl",
            "#!/bin/sh\nset -eu\nif [ -n \"${PNEUMA_ASSERT_CLOSED_DATABASE:-}\" ] && [ -f \"${PNEUMA_ASSERT_CLOSED_DATABASE}-journal\" ]; then\n    printf 'sqlite write transaction was open during an http effect\\n' >&2\n    exit 90\nfi\nprintf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_CURL_LOG\"\nprintf '%s' \"${PNEUMA_FAKE_CURL_STATUS:-200}\"\n",
        ),
    ] {
        let executable = fake_bin.join(name);
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).unwrap();
    }
}

fn make_executable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn executable_path(fake_bin: &Path) -> OsString {
    let inherited = env::var_os("PATH").unwrap_or_default();
    env::join_paths(std::iter::once(fake_bin.to_path_buf()).chain(env::split_paths(&inherited)))
        .unwrap()
}

fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_child(mut child: Child, timeout: Duration) -> Output {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "deploy child timed out\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
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
