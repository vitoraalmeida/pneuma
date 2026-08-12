use std::env;
use std::ffi::OsString;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::caddy_exposure::{
    CaddyCommandError, CaddyFilesystemAction, MaterializeCaddyFragmentError,
    canonical_fragment_contents, materialize_caddy_fragment, remove_caddy_fragment,
    restore_materialized_caddy_fragment,
};

const CHILD_CASE: &str = "PNEUMA_CADDY_TEST_CASE";
const MANAGED_DIRECTORY: &str = "PNEUMA_CADDY_TEST_MANAGED_DIRECTORY";
const CADDYFILE_PATH: &str = "PNEUMA_CADDY_TEST_CADDYFILE";
const CADDY_LOG: &str = "PNEUMA_CADDY_TEST_LOG";
const APPLICATION_ID: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn validates_the_complete_configuration_and_reloads_caddy() {
    let environment = CaddyTestEnvironment::new();
    environment.write_previous("old route\n");

    environment.run_child("success");

    let expected = "example.com {\n    reverse_proxy 127.0.0.1:31000\n}\n";
    assert_eq!(
        fs::read_to_string(environment.fragment_path()).unwrap(),
        expected
    );
    assert!(!environment.temporary_path().exists());
    assert_eq!(
        environment.caddy_commands(),
        vec![
            format!(
                "validate --config {} --adapter caddyfile",
                environment.temporary_path().display()
            ),
            format!(
                "validate --config {} --adapter caddyfile",
                environment.caddyfile_path.display()
            ),
            format!(
                "reload --config {} --adapter caddyfile",
                environment.caddyfile_path.display()
            ),
        ]
    );
}

#[test]
fn canonical_fragment_contents_changes_with_domain_or_endpoint() {
    let endpoint: SocketAddr = "127.0.0.1:31000".parse().unwrap();

    let baseline = canonical_fragment_contents("example.com", endpoint);

    assert_ne!(
        baseline,
        canonical_fragment_contents("other.example.com", endpoint)
    );
    assert_ne!(
        baseline,
        canonical_fragment_contents("example.com", "127.0.0.1:32000".parse().unwrap())
    );
}

#[test]
fn restores_a_successfully_applied_fragment_when_later_work_fails() {
    let environment = CaddyTestEnvironment::new();
    environment.write_previous("known good route\n");

    environment.run_child("success-and-restore");

    assert_eq!(environment.active_fragment(), "known good route\n");
    assert!(!environment.temporary_path().exists());
    assert_eq!(environment.caddy_commands().len(), 4);
}

#[test]
fn preserves_the_active_fragment_when_fragment_validation_fails() {
    let environment = CaddyTestEnvironment::new();
    environment.write_previous("known good route\n");

    environment.run_child("fragment-validation-failure");

    assert_eq!(environment.active_fragment(), "known good route\n");
    assert!(!environment.temporary_path().exists());
    assert_eq!(environment.caddy_commands().len(), 1);
}

#[test]
fn restores_the_active_fragment_when_complete_validation_fails() {
    let environment = CaddyTestEnvironment::new();
    environment.write_previous("known good route\n");

    environment.run_child("configuration-validation-failure");

    assert_eq!(environment.active_fragment(), "known good route\n");
    assert!(!environment.temporary_path().exists());
    assert_eq!(environment.caddy_commands().len(), 2);
}

#[test]
fn restores_and_reloads_the_active_fragment_when_candidate_reload_fails() {
    let environment = CaddyTestEnvironment::new();
    environment.write_previous("known good route\n");

    environment.run_child("reload-failure");

    assert_eq!(environment.active_fragment(), "known good route\n");
    assert!(!environment.temporary_path().exists());
    let commands = environment.caddy_commands();
    assert_eq!(commands.len(), 4);
    assert!(commands[2].starts_with("reload "));
    assert!(commands[3].starts_with("reload "));
}

#[test]
fn removes_a_new_fragment_when_its_first_reload_fails() {
    let environment = CaddyTestEnvironment::new();

    environment.run_child("reload-failure");

    assert!(!environment.fragment_path().exists());
    assert!(!environment.temporary_path().exists());
}

#[test]
fn restores_the_fragment_when_removal_reload_fails() {
    let environment = CaddyTestEnvironment::new();
    environment.write_previous("known good route\n");

    environment.run_removal_child("reload-failure");

    assert_eq!(environment.active_fragment(), "known good route\n");
    let commands = environment.caddy_commands();
    assert_eq!(commands.len(), 2);
    assert!(commands[0].starts_with("reload "));
    assert!(commands[1].starts_with("reload "));
}

#[test]
fn preserves_candidate_and_recovery_reload_diagnostics() {
    let environment = CaddyTestEnvironment::new();
    environment.write_previous("known good route\n");

    environment.run_child("recovery-reload-failure");

    assert_eq!(environment.active_fragment(), "known good route\n");
    assert!(!environment.temporary_path().exists());
}

#[test]
fn rejects_untrusted_fragment_coordinates_before_external_work() {
    let root = env::temp_dir().join(format!(
        "pneuma-caddy-invalid-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let caddyfile_path = root.join("Caddyfile");
    let loopback: SocketAddr = "127.0.0.1:31000".parse().unwrap();

    let invalid_application = materialize_caddy_fragment(
        &root,
        &caddyfile_path,
        "../application",
        "example.com",
        loopback,
    )
    .unwrap_err();
    let invalid_domain = materialize_caddy_fragment(
        &root,
        &caddyfile_path,
        APPLICATION_ID,
        "example..com",
        loopback,
    )
    .unwrap_err();
    let invalid_endpoint = materialize_caddy_fragment(
        &root,
        &caddyfile_path,
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
fn rejects_a_missing_main_caddyfile_before_creating_the_managed_directory() {
    let root = env::temp_dir().join(format!(
        "pneuma-caddy-missing-main-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let managed_directory = root.join("managed");
    let caddyfile_path = root.join("Caddyfile");

    let error = materialize_caddy_fragment(
        &managed_directory,
        &caddyfile_path,
        APPLICATION_ID,
        "example.com",
        "127.0.0.1:31000".parse().unwrap(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        MaterializeCaddyFragmentError::Filesystem {
            action: CaddyFilesystemAction::InspectCaddyfile,
            ..
        }
    ));
    assert!(!managed_directory.exists());
}

#[test]
fn caddy_child_process() {
    let Some(case) = env::var_os(CHILD_CASE) else {
        return;
    };
    let case = case.to_str().unwrap();
    let managed_directory = PathBuf::from(env::var_os(MANAGED_DIRECTORY).unwrap());
    let caddyfile_path = PathBuf::from(env::var_os(CADDYFILE_PATH).unwrap());
    let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, 31000));
    let result = materialize_caddy_fragment(
        &managed_directory,
        &caddyfile_path,
        APPLICATION_ID,
        "example.com",
        endpoint,
    );

    match case {
        "success" | "success-and-restore" => {
            let materialized = result.unwrap();
            assert_eq!(
                materialized.path,
                managed_directory.join(format!("{APPLICATION_ID}.caddy"))
            );
            assert_eq!(
                materialized.fragment_validation_stdout,
                "valid configuration\n"
            );
            assert_eq!(
                materialized.configuration_validation_stdout,
                "valid configuration\n"
            );
            assert_eq!(materialized.reload_stdout, "reload complete\n");
            if case == "success-and-restore" {
                restore_materialized_caddy_fragment(&materialized, &caddyfile_path).unwrap();
            }
        }
        "fragment-validation-failure" => {
            let error = result.unwrap_err();
            assert!(matches!(
                &error,
                MaterializeCaddyFragmentError::ValidateFragment {
                    failure: CaddyCommandError::Rejected { stderr, .. },
                }
                    if stderr.contains("invalid generated route")
            ));
        }
        "configuration-validation-failure" => {
            let error = result.unwrap_err();
            assert!(matches!(
                &error,
                MaterializeCaddyFragmentError::ValidateConfiguration {
                    failure: CaddyCommandError::Rejected { stderr, .. },
                    recovery: None,
                } if stderr.contains("invalid complete configuration")
            ));
        }
        "reload-failure" => {
            let error = result.unwrap_err();
            assert!(matches!(
                &error,
                MaterializeCaddyFragmentError::Reload {
                    failure: CaddyCommandError::Rejected { stderr, .. },
                    recovery: None,
                } if stderr.contains("candidate reload failed")
            ));
        }
        "recovery-reload-failure" => {
            let error = result.unwrap_err();
            let message = error.to_string();
            assert!(message.contains("candidate reload failed"));
            assert!(message.contains("recovery reload failed"));
            assert!(matches!(
                error,
                MaterializeCaddyFragmentError::Reload {
                    recovery: Some(_),
                    ..
                }
            ));
        }
        unknown => panic!("unknown child case: {unknown}"),
    }
}

#[test]
fn caddy_removal_child_process() {
    let Some(case) = env::var_os(CHILD_CASE) else {
        return;
    };
    let case = case.to_str().unwrap();
    if case != "removal-reload-failure" {
        return;
    }
    let managed_directory = PathBuf::from(env::var_os(MANAGED_DIRECTORY).unwrap());
    let caddyfile_path = PathBuf::from(env::var_os(CADDYFILE_PATH).unwrap());

    let error =
        remove_caddy_fragment(&managed_directory, APPLICATION_ID, &caddyfile_path).unwrap_err();
    assert!(matches!(
        error,
        pneuma::adapters::caddy_exposure::CaddyRecoveryError::Reload { .. }
    ));
}

struct CaddyTestEnvironment {
    root: PathBuf,
    managed_directory: PathBuf,
    caddyfile_path: PathBuf,
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
        let caddyfile_path = root.join("Caddyfile");
        let fake_bin = root.join("bin");
        let caddy_log = root.join("caddy.log");
        fs::create_dir_all(&fake_bin).unwrap();
        fs::write(
            &caddyfile_path,
            format!("import {}/*.caddy\n", managed_directory.display()),
        )
        .unwrap();
        install_fake_caddy(&fake_bin);
        Self {
            root,
            managed_directory,
            caddyfile_path,
            fake_bin,
            caddy_log,
        }
    }

    fn write_previous(&self, contents: &str) {
        fs::create_dir_all(&self.managed_directory).unwrap();
        fs::write(self.fragment_path(), contents).unwrap();
    }

    fn fragment_path(&self) -> PathBuf {
        self.managed_directory
            .join(format!("{APPLICATION_ID}.caddy"))
    }

    fn temporary_path(&self) -> PathBuf {
        self.managed_directory
            .join(format!(".{APPLICATION_ID}.caddy.tmp"))
    }

    fn active_fragment(&self) -> String {
        fs::read_to_string(self.fragment_path()).unwrap()
    }

    fn caddy_commands(&self) -> Vec<String> {
        fs::read_to_string(&self.caddy_log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn run_child(&self, case: &str) {
        let output = Command::new(env::current_exe().unwrap())
            .args(["--exact", "caddy_child_process", "--nocapture"])
            .env(CHILD_CASE, case)
            .env(MANAGED_DIRECTORY, &self.managed_directory)
            .env(CADDYFILE_PATH, &self.caddyfile_path)
            .env(CADDY_LOG, &self.caddy_log)
            .env("PNEUMA_FAKE_CADDY_CASE", case)
            .env(
                "PNEUMA_FAKE_CADDY_RELOAD_COUNT",
                self.root.join("reload-count"),
            )
            .env("PATH", executable_path(&self.fake_bin))
            .output()
            .unwrap();
        assert_command_succeeded(&output);
    }

    fn run_removal_child(&self, case: &str) {
        let output = Command::new(env::current_exe().unwrap())
            .args(["--exact", "caddy_removal_child_process", "--nocapture"])
            .env(CHILD_CASE, "removal-reload-failure")
            .env(MANAGED_DIRECTORY, &self.managed_directory)
            .env(CADDYFILE_PATH, &self.caddyfile_path)
            .env(CADDY_LOG, &self.caddy_log)
            .env("PNEUMA_FAKE_CADDY_CASE", case)
            .env(
                "PNEUMA_FAKE_CADDY_RELOAD_COUNT",
                self.root.join("reload-count"),
            )
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
printf '%s\n' "$*" >> "$PNEUMA_CADDY_TEST_LOG"

operation="$1"
configuration="$3"
case_name="$PNEUMA_FAKE_CADDY_CASE"

if [ "$operation" = "validate" ]; then
    if [ "$case_name" = "fragment-validation-failure" ] && [ "$configuration" != "$PNEUMA_CADDY_TEST_CADDYFILE" ]; then
        printf 'invalid generated route\n' >&2
        exit 1
    fi
    if [ "$case_name" = "configuration-validation-failure" ] && [ "$configuration" = "$PNEUMA_CADDY_TEST_CADDYFILE" ]; then
        printf 'invalid complete configuration\n' >&2
        exit 1
    fi
    printf 'valid configuration\n'
    exit 0
fi

count=0
if [ -f "$PNEUMA_FAKE_CADDY_RELOAD_COUNT" ]; then
    count=$(sed -n '1p' "$PNEUMA_FAKE_CADDY_RELOAD_COUNT")
fi
count=$((count + 1))
printf '%s\n' "$count" > "$PNEUMA_FAKE_CADDY_RELOAD_COUNT"
if [ "$case_name" = "reload-failure" ] && [ "$count" -eq 1 ]; then
    printf 'candidate reload failed\n' >&2
    exit 1
fi
if [ "$case_name" = "recovery-reload-failure" ]; then
    if [ "$count" -eq 1 ]; then
        printf 'candidate reload failed\n' >&2
    else
        printf 'recovery reload failed\n' >&2
    fi
    exit 1
fi
printf 'reload complete\n'
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
