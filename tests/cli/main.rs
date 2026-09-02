use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::net::{Ipv4Addr, TcpListener};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use pneuma::adapters::database::{self, DatabaseLock, LockMode};

mod exposure;
mod lifecycle;
mod reconciliation;
mod support;

use support::{
    DeploymentEnvironment, OciFailure, assert_command_succeeded, assert_identifier_line,
    create_repository_from_fixture, executable_path, fixture_path, initialize_repository,
    make_executable, respond_once, run_pneuma, run_pneuma_env, temporary_database_path,
    temporary_workspace_path, wait_for_child, wait_for_file,
};

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
fn import_without_a_required_system_fails_with_usage_exit_code() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = workspace.join("remote");
    fs::create_dir_all(&repository_path).unwrap();
    fs::write(
        repository_path.join("pneuma.toml"),
        concat!(
            "schema_version = 3\n",
            "\n",
            "[application]\n",
            "name = \"systemless-site\"\n",
            "\n",
            "[delivery]\n",
            "type = \"oci\"\n",
            "image = \"registry.example/team/service\"\n",
            "\n",
            "[runtime]\n",
            "container_port = 8080\n",
            "healthcheck_path = \"/healthz\"\n",
            "expected_status = 200\n",
            "\n",
            "[exposure]\n",
            "default_visibility = \"internal\"\n",
        ),
    )
    .unwrap();
    initialize_repository(&repository_path);
    let url = format!("file://{}", repository_path.display());

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("system is required"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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
