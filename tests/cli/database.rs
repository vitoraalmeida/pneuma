use std::ffi::OsStr;
use std::fs;
use std::process::Command;

use pneuma::adapters::database::{self, DatabaseLock, LockMode};

use crate::support::{
    DeploymentEnvironment, assert_command_succeeded, run_pneuma, temporary_database_path,
};

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
