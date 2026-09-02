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

pub(super) fn temporary_database_path() -> PathBuf {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!(
        "pneuma-cli-{}-{unique_suffix}.sqlite3",
        std::process::id()
    ))
}

pub(super) fn temporary_workspace_path() -> PathBuf {
    env::temp_dir().join(format!(
        "pneuma-cli-workspace-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
}

pub(super) fn create_repository_from_fixture(workspace: &Path, fixture: &str) -> PathBuf {
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

pub(super) fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub(super) fn run_pneuma_env(
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

pub(super) fn run_pneuma(database_path: &Path, arguments: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", database_path)
        .args(arguments)
        .output()
        .unwrap()
}

pub(super) struct DeploymentEnvironment {
    pub(super) root: PathBuf,
    repository_path: PathBuf,
    pub(super) database_path: PathBuf,
    pub(super) workspace_path: PathBuf,
    pub(super) fake_bin: PathBuf,
    pub(super) application_name: String,
    pub(super) managed_caddy_directory: PathBuf,
    pub(super) caddyfile_path: PathBuf,
    pub(super) image_repository: String,
    pub(super) stale_container_id: Option<String>,
    pub(super) replacement_container_id: Option<String>,
    pub(super) replacement_application_label: Option<String>,
    pub(super) reconciliation_port: Option<u16>,
    pub(super) reconciliation_curl_status: Option<u16>,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum OciFailure {
    Pull,
    DigestMismatch,
}

impl DeploymentEnvironment {
    pub(super) fn new() -> Self {
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

    pub(super) fn public() -> Self {
        Self::from_fixture("valid", "personal-site")
    }

    pub(super) fn deploy(&self, port: u16, verbose: bool) -> Output {
        self.deploy_with_external_status(port, verbose, 200)
    }

    pub(super) fn deploy_with_external_status(
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

    pub(super) fn deploy_command(&self, port: u16) -> Command {
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

    pub(super) fn ci_dispatch(&self, port: u16) -> Output {
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

    pub(super) fn deploy_current_revision(&self) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || respond_once(&listener, 200));
        assert_command_succeeded(&self.deploy(port, false));
        server.join().unwrap();
    }

    pub(super) fn spawn_gated_deploy(&self, port: u16, marker: &Path, release: &Path) -> Child {
        let mut command = self.deploy_command(port);
        command
            .env("PNEUMA_FAKE_SYSTEMCTL_START_MARKER", marker)
            .env("PNEUMA_FAKE_SYSTEMCTL_START_RELEASE", release)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().unwrap()
    }

    pub(super) fn deploy_oci(&self, reference: &str, port: u16) -> Output {
        self.run_oci_deploy(reference, port, None, None)
    }

    pub(super) fn run_oci_deploy(
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

    pub(super) fn deploy_oci_with_failure(&self, reference: &str, failure: OciFailure) -> Output {
        self.run_oci_deploy(reference, 30000, Some(failure), None)
    }

    pub(super) fn import(&self) -> Output {
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

    pub(super) fn commit(&self, contents: &str) {
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

    pub(super) fn run_lifecycle(&self, subcommand: &str) -> Output {
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

    pub(super) fn run_reconcile(&self) -> Output {
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

pub(super) fn initialize_repository(repository_path: &Path) {
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

pub(super) fn assert_command_succeeded(output: &Output) {
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

pub(super) fn respond_once(listener: &TcpListener, status: u16) {
    listener.set_nonblocking(true).unwrap();
    // The deadline must outlast the CLI's full internal health-check retry budget
    // (five 2-second attempts separated by 500 milliseconds), including the deploy
    // work that precedes the first attempt; a shorter deadline closes the listener
    // before the CLI connects and turns a slow pre-check phase into a spurious
    // connection-refused failure.
    let deadline = Instant::now() + Duration::from_secs(15);
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
        if [ -n "${PNEUMA_FAKE_PODMAN_PORT:-}" ]; then
            printf '%s\n' "$PNEUMA_FAKE_PODMAN_PORT"
        else
            printf '127.0.0.1:%s\n' "$PNEUMA_FAKE_PORT"
        fi
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
            r#"#!/bin/sh
set -eu
if [ -n "${PNEUMA_ASSERT_CLOSED_DATABASE:-}" ] && [ -f "${PNEUMA_ASSERT_CLOSED_DATABASE}-journal" ]; then
    printf 'sqlite write transaction was open during a caddy effect\n' >&2
    exit 90
fi
if [ -f "${PNEUMA_FAKE_CADDY_FAILURE:-}" ]; then
    printf 'caddy failure injected\n' >&2
    exit 1
fi
case "$1" in
    validate) printf 'valid configuration\n' ;;
    reload) printf 'reload complete\n' ;;
    *) exit 1 ;;
esac
"#,
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

pub(super) fn make_executable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

pub(super) fn executable_path(fake_bin: &Path) -> OsString {
    let inherited = env::var_os("PATH").unwrap_or_default();
    env::join_paths(std::iter::once(fake_bin.to_path_buf()).chain(env::split_paths(&inherited)))
        .unwrap()
}

pub(super) fn wait_for_file(path: &Path, timeout: Duration) {
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

pub(super) fn wait_for_child(mut child: Child, timeout: Duration) -> Output {
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

pub(super) fn assert_identifier_line(line: &str, prefix: &str) {
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
