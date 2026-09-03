use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::host_environment::{HOST_ENVIRONMENT_FILE_VARIABLE, parse_host_environment};

#[test]
fn parse_accepts_comments_whitespace_empty_values_and_extra_separators() {
    let entries = parse_host_environment(
        "# managed by bootstrap\n\n  DATABASE = /var/lib/pneuma/database/pneuma.sqlite3  \nEMPTY=\nEQUATION=one=two#three\n",
    )
    .unwrap();

    assert_eq!(
        entries,
        vec![
            (
                "DATABASE".to_owned(),
                "/var/lib/pneuma/database/pneuma.sqlite3".to_owned()
            ),
            ("EMPTY".to_owned(), String::new()),
            ("EQUATION".to_owned(), "one=two#three".to_owned()),
        ]
    );
}

#[test]
fn a_missing_host_environment_file_boots_normally() {
    let missing = temporary_path("missing-environment");
    let output = run_pneuma(
        &[(HOST_ENVIRONMENT_FILE_VARIABLE, &missing.to_string_lossy())],
        &[],
        &["version"],
    );

    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
}

#[test]
fn an_unreadable_host_environment_file_fails_startup() {
    let directory = temporary_path("unreadable-environment");
    fs::create_dir_all(&directory).unwrap();
    let output = run_pneuma(
        &[(HOST_ENVIRONMENT_FILE_VARIABLE, &directory.to_string_lossy())],
        &[],
        &["version"],
    );
    let _ = fs::remove_dir_all(&directory);

    assert_startup_failure(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot read host environment file"),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_invalid_utf8_host_environment_file_fails_startup_without_creating_the_database() {
    let file = temporary_path("invalid-utf8-environment");
    fs::write(&file, b"DATABASE=\xff\xfe\n").unwrap();
    let database_path = temporary_database_path();
    let output = run_pneuma(
        &[
            (HOST_ENVIRONMENT_FILE_VARIABLE, &file.to_string_lossy()),
            ("PNEUMA_DATABASE_PATH", &database_path.to_string_lossy()),
        ],
        &[],
        &["app", "list"],
    );
    let _ = fs::remove_file(&file);

    assert_startup_failure(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not valid UTF-8"),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!database_path.exists());
}

#[test]
fn valid_entries_are_applied_including_the_file_provided_database_path() {
    let file = temporary_path("valid-environment");
    let database_path = temporary_database_path();
    fs::write(
        &file,
        format!(
            "# pneuma test environment\n\nPNEUMA_DATABASE_PATH={}\n  SPACED_NAME = spaced value  \nEMPTY_VALUE=\nEQUATION=one=two#three\n",
            database_path.display()
        ),
    )
    .unwrap();
    let output = run_pneuma(
        &[(HOST_ENVIRONMENT_FILE_VARIABLE, &file.to_string_lossy())],
        &[],
        &["app", "list"],
    );
    let _ = fs::remove_file(&file);

    assert!(output.status.success());
    assert!(
        database_path.exists(),
        "file-provided database path was not used"
    );
    let _ = fs::remove_file(&database_path);
}

#[test]
fn caller_environment_values_override_file_values() {
    let file = temporary_path("precedence-environment");
    let file_database_path = temporary_database_path();
    let caller_database_path = temporary_database_path();
    fs::write(
        &file,
        format!("PNEUMA_DATABASE_PATH={}\n", file_database_path.display()),
    )
    .unwrap();
    let output = run_pneuma(
        &[
            (HOST_ENVIRONMENT_FILE_VARIABLE, &file.to_string_lossy()),
            (
                "PNEUMA_DATABASE_PATH",
                &caller_database_path.to_string_lossy(),
            ),
        ],
        &[],
        &["app", "list"],
    );
    let _ = fs::remove_file(&file);

    assert!(output.status.success());
    assert!(
        caller_database_path.exists(),
        "caller-supplied database path was not used"
    );
    assert!(!file_database_path.exists());
    let _ = fs::remove_file(&caller_database_path);
}

#[test]
fn a_malformed_host_environment_file_fails_startup_with_the_line_number() {
    let file = temporary_path("malformed-environment");
    fs::write(&file, "PNEUMA_DATABASE_PATH=/tmp/ignored\nBROKEN\n").unwrap();
    let database_path = temporary_database_path();
    let output = run_pneuma(
        &[
            (HOST_ENVIRONMENT_FILE_VARIABLE, &file.to_string_lossy()),
            ("PNEUMA_DATABASE_PATH", &database_path.to_string_lossy()),
        ],
        &[],
        &["app", "list"],
    );
    let _ = fs::remove_file(&file);

    assert_startup_failure(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("line 2"),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!database_path.exists());
}

#[test]
fn invalid_variable_names_fail_startup() {
    for content in ["1NAME=value\n", "=value\n", "WITH-DASH=value\n"] {
        let file = temporary_path("invalid-name-environment");
        fs::write(&file, content).unwrap();
        let output = run_pneuma(
            &[(HOST_ENVIRONMENT_FILE_VARIABLE, &file.to_string_lossy())],
            &[],
            &["version"],
        );
        let _ = fs::remove_file(&file);

        assert_startup_failure(&output);
    }
}

#[test]
fn a_nul_byte_in_a_value_fails_startup() {
    let file = temporary_path("nul-environment");
    fs::write(&file, b"NAME=value\0suffix\n").unwrap();
    let output = run_pneuma(
        &[(HOST_ENVIRONMENT_FILE_VARIABLE, &file.to_string_lossy())],
        &[],
        &["version"],
    );
    let _ = fs::remove_file(&file);

    assert_startup_failure(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("NUL byte"),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn duplicate_variables_fail_startup_with_both_line_numbers() {
    let file = temporary_path("duplicate-environment");
    fs::write(&file, "NAME=first\nOTHER=x\nNAME=second\n").unwrap();
    let output = run_pneuma(
        &[(HOST_ENVIRONMENT_FILE_VARIABLE, &file.to_string_lossy())],
        &[],
        &["version"],
    );
    let _ = fs::remove_file(&file);

    assert_startup_failure(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("duplicate host environment variable `NAME` on lines 1 and 3"),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_late_parse_failure_applies_no_entries() {
    let file = temporary_path("partial-application-environment");
    let database_path = temporary_database_path();
    fs::write(
        &file,
        format!("PNEUMA_DATABASE_PATH={}\nBROKEN\n", database_path.display()),
    )
    .unwrap();
    let output = run_pneuma(
        &[(HOST_ENVIRONMENT_FILE_VARIABLE, &file.to_string_lossy())],
        &[],
        &["app", "list"],
    );
    let _ = fs::remove_file(&file);

    assert_startup_failure(&output);
    assert!(
        !database_path.exists(),
        "entries were applied despite a late parse failure"
    );
}

#[test]
fn startup_requires_a_nonempty_home_or_quadlet_directory() {
    let missing = temporary_path("quadlet-requirement-environment");
    let file = missing.to_string_lossy().into_owned();

    let without_both = run_pneuma(
        &[(HOST_ENVIRONMENT_FILE_VARIABLE, &file)],
        &["HOME", "PNEUMA_QUADLET_DIR"],
        &["version"],
    );
    assert_startup_failure(&without_both);
    assert!(
        String::from_utf8_lossy(&without_both.stderr)
            .contains("either HOME or PNEUMA_QUADLET_DIR must be set to a nonempty value"),
        "stderr was: {}",
        String::from_utf8_lossy(&without_both.stderr)
    );

    let with_empty_home = run_pneuma(
        &[(HOST_ENVIRONMENT_FILE_VARIABLE, &file), ("HOME", "")],
        &["PNEUMA_QUADLET_DIR"],
        &["version"],
    );
    assert_startup_failure(&with_empty_home);

    let with_quadlet_only = run_pneuma(
        &[
            (HOST_ENVIRONMENT_FILE_VARIABLE, &file),
            ("PNEUMA_QUADLET_DIR", "/tmp/pneuma-test-quadlet"),
        ],
        &["HOME"],
        &["version"],
    );
    assert!(with_quadlet_only.status.success());
}

fn assert_startup_failure(output: &Output) {
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "stdout was not empty");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 1, "stderr was: {stderr}");
    assert!(lines[0].starts_with("error: "), "stderr was: {stderr}");
}

fn temporary_database_path() -> PathBuf {
    temporary_path("database").with_extension("sqlite3")
}

fn temporary_path(purpose: &str) -> PathBuf {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pneuma-host-environment-{purpose}-{}-{unique_suffix}",
        std::process::id()
    ))
}

fn run_pneuma(environment: &[(&str, &str)], removals: &[&str], arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
    for (name, value) in environment {
        command.env(name, value);
    }
    for name in removals {
        command.env_remove(name);
    }
    command.args(arguments).output().unwrap()
}
