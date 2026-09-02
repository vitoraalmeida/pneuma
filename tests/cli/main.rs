use std::ffi::OsStr;
use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::process::Command;
use std::thread;

use pneuma::adapters::database::{self, DatabaseLock, LockMode};

mod deployment;
mod exposure;
mod lifecycle;
mod reconciliation;
mod support;

use support::{
    DeploymentEnvironment, OciFailure, assert_command_succeeded, create_repository_from_fixture,
    executable_path, fixture_path, initialize_repository, make_executable, respond_once,
    run_pneuma, run_pneuma_env, temporary_database_path, temporary_workspace_path,
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
