use std::env;
use std::ffi::OsString;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::caddy_exposure::{MaterializeCaddyFragmentError, materialize_caddy_fragment};

const CHILD_CASE: &str = "PNEUMA_CADDY_TEST_CASE";
const MANAGED_DIRECTORY: &str = "PNEUMA_CADDY_TEST_MANAGED_DIRECTORY";
const CADDY_LOG: &str = "PNEUMA_CADDY_TEST_LOG";
const APPLICATION_ID: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn validates_and_atomically_replaces_a_managed_fragment() {
    let environment = CaddyTestEnvironment::new();
    fs::create_dir(&environment.managed_directory).unwrap();
    fs::write(environment.fragment_path(), "old route\n").unwrap();

    environment.run_child("success", false);

    let expected = "example.com {\n    reverse_proxy 127.0.0.1:31000\n}\n";
    assert_eq!(
        fs::read_to_string(environment.fragment_path()).unwrap(),
        expected
    );
    assert!(!environment.temporary_path().exists());
    assert_eq!(
        fs::read_to_string(&environment.caddy_log).unwrap(),
        format!(
            "validate --config {} --adapter caddyfile\n",
            environment.temporary_path().display()
        )
    );
}

#[test]
fn preserves_the_active_fragment_when_caddy_rejects_the_temporary_file() {
    let environment = CaddyTestEnvironment::new();
    fs::create_dir(&environment.managed_directory).unwrap();
    fs::write(environment.fragment_path(), "known good route\n").unwrap();

    environment.run_child("validation-failure", true);

    assert_eq!(
        fs::read_to_string(environment.fragment_path()).unwrap(),
        "known good route\n"
    );
    assert!(!environment.temporary_path().exists());
}

#[test]
fn rejects_untrusted_fragment_coordinates_before_external_work() {
    let root = env::temp_dir().join(format!(
        "pneuma-caddy-invalid-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let loopback: SocketAddr = "127.0.0.1:31000".parse().unwrap();

    let invalid_application =
        materialize_caddy_fragment(&root, "../application", "example.com", loopback).unwrap_err();
    let invalid_domain =
        materialize_caddy_fragment(&root, APPLICATION_ID, "example..com", loopback).unwrap_err();
    let invalid_endpoint = materialize_caddy_fragment(
        &root,
        APPLICATION_ID,
        "example.com",
        "0.0.0.0:31000".parse().unwrap(),
    )
    .unwrap_err();

    assert!(matches!(
        invalid_application,
        MaterializeCaddyFragmentError::InvalidApplicationId
    ));
    assert!(matches!(
        invalid_domain,
        MaterializeCaddyFragmentError::InvalidDomain
    ));
    assert!(matches!(
        invalid_endpoint,
        MaterializeCaddyFragmentError::InvalidEndpoint { .. }
    ));
    assert!(!root.exists());
}

#[test]
fn caddy_child_process() {
    let Some(case) = env::var_os(CHILD_CASE) else {
        return;
    };
    let managed_directory = PathBuf::from(env::var_os(MANAGED_DIRECTORY).unwrap());
    let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, 31000));

    match case.to_str().unwrap() {
        "success" => {
            let materialized = materialize_caddy_fragment(
                &managed_directory,
                APPLICATION_ID,
                "example.com",
                endpoint,
            )
            .unwrap();
            assert_eq!(
                materialized.path,
                managed_directory.join(format!("{APPLICATION_ID}.caddy"))
            );
            assert_eq!(materialized.validation_stdout, "valid configuration\n");
        }
        "validation-failure" => {
            let error = materialize_caddy_fragment(
                &managed_directory,
                APPLICATION_ID,
                "example.com",
                endpoint,
            )
            .unwrap_err();
            assert!(error.to_string().contains("invalid generated route"));
            assert!(matches!(
                &error,
                MaterializeCaddyFragmentError::ValidationFailed { stderr, .. }
                    if stderr.contains("invalid generated route")
            ));
        }
        unknown => panic!("unknown child case: {unknown}"),
    }
}

struct CaddyTestEnvironment {
    root: PathBuf,
    managed_directory: PathBuf,
    fake_bin: PathBuf,
    caddy_log: PathBuf,
}

impl CaddyTestEnvironment {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "pneuma-caddy-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let managed_directory = root.join("managed");
        let fake_bin = root.join("bin");
        let caddy_log = root.join("caddy.log");
        fs::create_dir_all(&fake_bin).unwrap();
        install_fake_caddy(&fake_bin);
        Self {
            root,
            managed_directory,
            fake_bin,
            caddy_log,
        }
    }

    fn fragment_path(&self) -> PathBuf {
        self.managed_directory
            .join(format!("{APPLICATION_ID}.caddy"))
    }

    fn temporary_path(&self) -> PathBuf {
        self.managed_directory
            .join(format!(".{APPLICATION_ID}.caddy.tmp"))
    }

    fn run_child(&self, case: &str, validation_failure: bool) {
        let output = Command::new(env::current_exe().unwrap())
            .args(["--exact", "caddy_child_process", "--nocapture"])
            .env(CHILD_CASE, case)
            .env(MANAGED_DIRECTORY, &self.managed_directory)
            .env(CADDY_LOG, &self.caddy_log)
            .env("PNEUMA_FAKE_CADDY_FAILURE", validation_failure.to_string())
            .env("PATH", executable_path(&self.fake_bin))
            .output()
            .unwrap();
        assert_command_succeeded(&output);
    }
}

impl Drop for CaddyTestEnvironment {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn install_fake_caddy(fake_bin: &Path) {
    let caddy = fake_bin.join("caddy");
    fs::write(
        &caddy,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" > "$PNEUMA_CADDY_TEST_LOG"
if [ "$PNEUMA_FAKE_CADDY_FAILURE" = "true" ]; then
    printf 'invalid generated route\n' >&2
    exit 1
fi
printf 'valid configuration\n'
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&caddy).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(caddy, permissions).unwrap();
}

fn executable_path(fake_bin: &Path) -> OsString {
    let inherited = env::var_os("PATH").unwrap_or_default();
    env::join_paths(std::iter::once(fake_bin.to_path_buf()).chain(env::split_paths(&inherited)))
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
