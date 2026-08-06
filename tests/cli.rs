use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::database;

#[test]
fn imports_and_lists_an_application_idempotently() {
    let database_path = temporary_database_path();
    let repository_path = fixture_path("valid");

    let first_import = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    );
    let second_import = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    );
    let list = run_pneuma(&database_path, &[OsStr::new("app"), OsStr::new("list")]);
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
    let repository_path = fixture_path("missing");

    let output = run_pneuma(
        &database_path,
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing/pneuma.toml"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reports_database_open_errors_and_returns_failure() {
    let database_path = temporary_database_path()
        .join("missing")
        .join("pneuma.sqlite3");

    let output = run_pneuma(&database_path, &[OsStr::new("app"), OsStr::new("list")]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to open database at"));
    assert!(stderr.contains(database_path.to_string_lossy().as_ref()));
}

#[test]
fn reports_usage_for_an_unknown_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .args(["unknown"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("[--verbose]"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("[--verbose] app import"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("[--verbose] app list"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("[--verbose] app deploy"));
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
    assert_eq!(lines[1], format!("Commit: {}", environment.commit_sha));
    assert_identifier_line(lines[2], "Deployment: ");
    assert_identifier_line(lines[3], "Runtime: ");
    assert_eq!(
        lines[4],
        format!("Container: pneuma-another-site-{}", environment.commit_sha)
    );
    assert_eq!(lines[5], "Status: Succeeded");
    assert_eq!(
        fs::read_dir(&environment.workspace_path).unwrap().count(),
        1
    );
}

#[test]
fn verbose_deployment_reports_steps_and_persisted_states() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));

    let output = environment.deploy(port, true);
    server.join().unwrap();

    assert_command_succeeded(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Status: Succeeded"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "[verbose] database:",
        "[verbose] deployment input:",
        "resolve Git revision: started",
        "resolve Git revision: completed",
        "prepare checkout: completed",
        "build image: completed",
        "create candidate container: completed",
        "start candidate container: completed",
        "observe candidate container: completed",
        "register candidate runtime: completed",
        "health check and promotion: completed",
        "state changed to Pending",
        "state changed to PreparingSource",
        "state changed to Building",
        "state changed to Starting",
        "state changed to VerifyingInternal",
        "state changed to Succeeded",
    ] {
        assert!(
            stderr.contains(expected),
            "missing `{expected}` in:\n{stderr}"
        );
    }
}

#[test]
fn verbose_redeployment_reports_current_runtime_reconciliation() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        respond_once(&listener, 200);
        respond_once(&listener, 200);
    });
    assert_command_succeeded(&environment.deploy(port, false));
    let output = environment.deploy(port, true);
    server.join().unwrap();

    assert_command_succeeded(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("reconcile existing runtime: started"));
    assert!(stderr.contains("reconcile existing runtime: completed"));
    assert!(!stderr.contains("build image: started"));
}

#[test]
fn verbose_failure_reports_persistence_and_candidate_cleanup() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for _ in 0..5 {
            respond_once(&listener, 503);
        }
    });

    let output = environment.deploy(port, true);
    server.join().unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failure persisted (health_check_failed)"));
    assert!(stderr.contains("clean up candidate: started"));
    assert!(stderr.contains("clean up candidate: completed"));
    assert!(stderr.contains("error: deployment"));
}

#[test]
fn reports_a_missing_application_before_external_work() {
    let database_path = temporary_database_path();
    let workspace_path = database_path.with_extension("workspaces");

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .env("PNEUMA_WORKSPACE_PATH", &workspace_path)
        .args([
            "app",
            "deploy",
            "missing-application",
            "missing-repository",
            "--revision",
            "main",
        ])
        .output()
        .unwrap();
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("application `missing-application` was not found")
    );
    assert!(!workspace_path.exists());
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("internal health check: completed"));
    assert!(stderr.contains("apply public route: completed"));
    assert!(stderr.contains("external health check: completed"));
    let curl_command = fs::read_to_string(environment.root.join("curl.log")).unwrap();
    assert!(curl_command.contains("--resolve vitoralmeida.tech:443:127.0.0.1"));
    assert!(curl_command.contains("https://vitoralmeida.tech/healthz"));
    let connection = database::open(&environment.database_path).unwrap();
    let state: (String, String, String, String) = connection
        .query_row(
            "SELECT exposures.materialization_state,
                    exposures.configuration_version,
                    runtime_instances.role,
                    deployments.status
             FROM exposures
             JOIN runtime_instances ON runtime_instances.id = exposures.active_runtime_id
             JOIN deployments ON deployments.id = runtime_instances.deployment_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        state,
        (
            "active".to_owned(),
            environment.commit_sha.clone(),
            "current".to_owned(),
            "succeeded".to_owned(),
        )
    );
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
    assert_command_succeeded(&environment.deploy(first_port, false));
    first_server.join().unwrap();
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
    let second_server = thread::spawn(move || respond_once(&second_listener, 200));

    let output = environment.deploy_with_external_status(second_port, false, 503);
    second_server.join().unwrap();

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
            "SELECT id, role FROM runtime_instances
             WHERE id = ?1 AND removed_at IS NULL",
            [&first_active_runtime],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(active_runtime.1, "current");
}

#[test]
fn accepts_native_repository_paths_but_requires_textual_name_and_revision() {
    let database_path = temporary_database_path();
    let invalid_utf8 = OsString::from_vec(vec![0xff]);
    let native_path = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["app", "deploy", "missing-application"])
        .arg(&invalid_utf8)
        .args(["--revision", "main"])
        .output()
        .unwrap();
    let invalid_name = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["app", "deploy"])
        .arg(&invalid_utf8)
        .args(["repository", "--revision", "main"])
        .output()
        .unwrap();
    let invalid_revision = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args([
            "app",
            "deploy",
            "missing-application",
            "repository",
            "--revision",
        ])
        .arg(invalid_utf8)
        .output()
        .unwrap();
    let _ = fs::remove_file(&database_path);

    assert!(
        String::from_utf8_lossy(&native_path.stderr)
            .contains("application `missing-application` was not found")
    );
    for output in [invalid_name, invalid_revision] {
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
    }
}

fn run_pneuma(database_path: &Path, arguments: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", database_path)
        .args(arguments)
        .output()
        .unwrap()
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

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
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
    commit_sha: String,
}

impl DeploymentEnvironment {
    fn new() -> Self {
        Self::from_fixture("another", "another-site")
    }

    fn public() -> Self {
        Self::from_fixture("valid", "personal-site")
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
        install_fake_caddy_and_curl(&fake_bin);
        fs::write(
            &caddyfile_path,
            format!("import {}/*.caddy\n", managed_caddy_directory.display()),
        )
        .unwrap();
        let commit_sha = git(&repository_path, &["rev-parse", "HEAD"])
            .trim()
            .to_owned();

        Self {
            root,
            repository_path,
            database_path,
            workspace_path,
            fake_bin,
            application_name: application_name.to_owned(),
            managed_caddy_directory,
            caddyfile_path,
            commit_sha,
        }
    }

    fn import(&self) -> Output {
        run_pneuma(
            &self.database_path,
            &[
                OsStr::new("app"),
                OsStr::new("import"),
                self.repository_path.as_os_str(),
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

    fn deploy(&self, port: u16, verbose: bool) -> Output {
        self.deploy_with_external_status(port, verbose, 200)
    }

    fn deploy_with_external_status(
        &self,
        port: u16,
        verbose: bool,
        external_status: u16,
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pneuma"));
        command
            .env("PNEUMA_DATABASE_PATH", &self.database_path)
            .env("PNEUMA_WORKSPACE_PATH", &self.workspace_path)
            .env("PNEUMA_CADDY_MANAGED_PATH", &self.managed_caddy_directory)
            .env("PNEUMA_CADDYFILE_PATH", &self.caddyfile_path)
            .env("PATH", executable_path(&self.fake_bin))
            .env("PNEUMA_FAKE_PORT", port.to_string())
            .env("PNEUMA_FAKE_PODMAN_COUNT", self.root.join("podman-count"))
            .env("PNEUMA_FAKE_CURL_LOG", self.root.join("curl.log"))
            .env("PNEUMA_FAKE_CURL_STATUS", external_status.to_string());
        if verbose {
            command.arg("--verbose");
        }
        command
            .args(["app", "deploy", &self.application_name])
            .arg(&self.repository_path)
            .args(["--revision", "HEAD"])
            .output()
            .unwrap()
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

fn install_fake_podman(fake_bin: &Path) {
    let podman = fake_bin.join("podman");
    fs::write(
        &podman,
        r#"#!/bin/sh
set -eu

case "$1" in
    build|start|container)
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
        printf 'running\n'
        ;;
    port)
        printf '127.0.0.1:%s\n' "$PNEUMA_FAKE_PORT"
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

fn install_fake_caddy_and_curl(fake_bin: &Path) {
    for (name, script) in [
        (
            "caddy",
            "#!/bin/sh\nset -eu\ncase \"$1\" in validate) printf 'valid configuration\\n' ;; reload) printf 'reload complete\\n' ;; *) exit 1 ;; esac\n",
        ),
        (
            "curl",
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_CURL_LOG\"\nprintf '%s' \"${PNEUMA_FAKE_CURL_STATUS:-200}\"\n",
        ),
    ] {
        let executable = fake_bin.join(name);
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).unwrap();
    }
}

fn executable_path(fake_bin: &Path) -> OsString {
    let inherited = env::var_os("PATH").unwrap_or_default();
    env::join_paths(std::iter::once(fake_bin.to_path_buf()).chain(env::split_paths(&inherited)))
        .unwrap()
}

fn respond_once(listener: &TcpListener, status: u16) {
    let (mut stream, _) = listener.accept().unwrap();
    read_request(&mut stream);
    let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n");
    stream.write_all(response.as_bytes()).unwrap();
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

fn assert_identifier_line(line: &str, prefix: &str) {
    let identifier = line.strip_prefix(prefix).unwrap();
    assert_eq!(identifier.len(), 32);
    assert!(identifier.bytes().all(|byte| byte.is_ascii_hexdigit()));
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
