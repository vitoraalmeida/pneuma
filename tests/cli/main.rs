use std::ffi::OsStr;
use std::fs;
use std::process::Command;

mod catalog;
mod database;
mod deployment;
mod exposure;
mod lifecycle;
mod reconciliation;
mod support;

use support::{
    DeploymentEnvironment, OciFailure, assert_command_succeeded, executable_path, make_executable,
    run_pneuma, temporary_database_path, temporary_workspace_path,
};

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
