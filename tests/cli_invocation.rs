use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_database_path() -> PathBuf {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pneuma-cli-invocation-{}-{unique_suffix}.sqlite3",
        std::process::id()
    ))
}

fn assert_database_was_not_created(database_path: &PathBuf) {
    assert!(
        !database_path.exists(),
        "the invocation must not create the database at {}",
        database_path.display()
    );
    let _ = fs::remove_file(database_path);
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
fn direct_version_prints_the_exact_release_line_without_touching_the_database() {
    let database_path = temporary_database_path();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["version"])
        .output()
        .unwrap();
    assert_database_was_not_created(&database_path);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("pneuma {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn ci_dispatched_version_prints_the_exact_release_line_without_touching_the_database() {
    let database_path = temporary_database_path();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .env("SSH_ORIGINAL_COMMAND", "version")
        .args(["ci", "dispatch"])
        .output()
        .unwrap();
    assert_database_was_not_created(&database_path);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("pneuma {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn ci_dispatch_fails_without_ssh_original_command_and_creates_no_database() {
    let database_path = temporary_database_path();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["ci", "dispatch"])
        .output()
        .unwrap();
    assert_database_was_not_created(&database_path);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: SSH_ORIGINAL_COMMAND not set\n"
    );
    assert!(output.stdout.is_empty());
}
