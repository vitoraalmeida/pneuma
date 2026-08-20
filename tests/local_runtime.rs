use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use pneuma::adapters::local_runtime::{
    ControlContainerError, CreateContainerError, create_container, observe_container,
    start_container, stop_container,
};
use pneuma::domain::runtime::ObservedRuntimeState;

#[test]
fn creates_a_container_without_a_stray_label_flag() {
    let environment = FakePodman::new();
    let commit_sha = "a".repeat(40);
    let reference = format!("localhost/pneuma/personal-site:{commit_sha}");

    let created =
        environment.run(|| create_container(&reference, "personal-site", &commit_sha, 8080));

    let commands = fs::read_to_string(environment.log_path()).unwrap();
    assert_eq!(
        commands.trim(),
        format!(
            "create --pull=never --name pneuma-personal-site-{commit_sha} \
             --label io.pneuma.application=personal-site \
             --label io.pneuma.revision={commit_sha} \
             --publish 127.0.0.1::8080 {reference}"
        )
    );
    assert!(created.unwrap().id.len() == 64);
}

struct FakePodman {
    root: PathBuf,
    bin: PathBuf,
    log_path: PathBuf,
}

impl FakePodman {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "pneuma-local-runtime-fake-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let log_path = root.join("podman.log");
        let podman = bin.join("podman");
        fs::write(
            &podman,
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_PODMAN_LOG\"\nprintf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&podman).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&podman, permissions).unwrap();
        Self {
            root,
            bin,
            log_path,
        }
    }

    fn run<T>(&self, operation: impl FnOnce() -> T) -> T {
        let _lock = environment_lock().lock().unwrap();
        let path = env::join_paths(
            std::iter::once(self.bin.clone())
                .chain(env::split_paths(&env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let previous_path = env::var_os("PATH");
        unsafe { env::set_var("PATH", path) };
        unsafe { env::set_var("PNEUMA_FAKE_PODMAN_LOG", &self.log_path) };
        let result = operation();
        if let Some(path) = previous_path {
            unsafe { env::set_var("PATH", path) };
        }
        result
    }

    fn log_path(&self) -> PathBuf {
        self.log_path.clone()
    }
}

impl Drop for FakePodman {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn environment_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
#[ignore = "requires configured rootless Podman"]
fn creates_controls_and_observes_a_rootless_candidate() {
    assert_eq!(
        podman(&["info", "--format", "{{.Host.Security.Rootless}}"]).trim(),
        "true"
    );

    let temporary_directory = TemporaryDirectory::new();
    let runtime_source = temporary_directory.path.join("runtime.rs");
    let runtime_process = temporary_directory.path.join("runtime-process");
    fs::write(
        &runtime_source,
        r#"use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let listener = TcpListener::bind("0.0.0.0:8080").unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut request = [0; 1024];
        let bytes_read = stream.read(&mut request).unwrap();
        let status = if request[..bytes_read].starts_with(b"GET /healthz ") {
            "200 OK"
        } else {
            "404 Not Found"
        };
        let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
        stream.write_all(response.as_bytes()).unwrap();
    }
}
"#,
    )
    .unwrap();
    let compile = Command::new("rustc")
        .args(["-C", "target-feature=+crt-static", "-C", "opt-level=s"])
        .arg(&runtime_source)
        .arg("-o")
        .arg(&runtime_process)
        .output()
        .unwrap();
    assert_command_succeeded(&compile);
    fs::write(
        temporary_directory.path.join("Containerfile"),
        "FROM scratch\nCOPY runtime-process /runtime-process\nENTRYPOINT [\"/runtime-process\"]\n",
    )
    .unwrap();
    let commit_sha = format!("{:040x}", unique_suffix());
    let reference = format!("localhost/pneuma/pneuma-runtime-test:{commit_sha}");
    let build_output = Command::new("podman")
        .args(["build", "--tag", &reference, "--file", "Containerfile", "."])
        .current_dir(&temporary_directory.path)
        .output()
        .unwrap();
    assert_command_succeeded(&build_output);
    let image = TestImage::new(reference.clone());

    let created = create_container(&reference, "pneuma-runtime-test", &commit_sha, 8080).unwrap();
    let container = TestContainer::new(created.id.clone());

    assert_eq!(
        inspect(&container.id, "{{.Name}}"),
        format!("pneuma-pneuma-runtime-test-{commit_sha}")
    );
    assert_eq!(
        inspect(
            &container.id,
            "{{ index .Config.Labels \"io.pneuma.application\" }}"
        ),
        "pneuma-runtime-test"
    );
    assert_eq!(
        inspect(
            &container.id,
            "{{ index .Config.Labels \"io.pneuma.revision\" }}"
        ),
        commit_sha
    );
    assert_eq!(
        inspect(
            &container.id,
            "{{ index .Config.Labels \"io.pneuma.application\" }}"
        ),
        "candidate"
    );
    assert_eq!(inspect(&container.id, "{{.State.Running}}"), "false");
    assert_eq!(
        inspect(&container.id, "{{.HostConfig.Privileged}}"),
        "false"
    );
    assert_eq!(inspect(&container.id, "{{json .Mounts}}"), "[]");

    let port_bindings = inspect(&container.id, "{{json .HostConfig.PortBindings}}");
    assert!(
        port_bindings.contains("8080/tcp"),
        "bindings: {port_bindings}"
    );
    assert!(
        port_bindings.contains("127.0.0.1"),
        "bindings: {port_bindings}"
    );

    let error = create_container(&reference, "pneuma-runtime-test", &commit_sha, 8080).unwrap_err();
    assert!(matches!(
        error,
        CreateContainerError::Create {
            ref stdout,
            ref stderr,
            ..
        } if !stdout.is_empty() || !stderr.is_empty()
    ));

    assert_eq!(
        observe_container(&container.id, 8080).unwrap(),
        pneuma::domain::runtime::ContainerObservation::not_running(ObservedRuntimeState::Created,)
            .unwrap()
    );

    start_container(&container.id).unwrap();
    start_container(&container.id).unwrap();
    let running = observe_container(&container.id, 8080).unwrap();
    assert_eq!(running.state(), &ObservedRuntimeState::Running);
    let endpoint = running
        .observed_endpoint()
        .expect("running container needs an endpoint");
    assert!(endpoint.ip().is_loopback());
    assert_ne!(endpoint.port(), 0);
    assert_eq!(
        check_internal_health(endpoint, "/healthz", 200).unwrap(),
        HealthCheckResult::Healthy {
            attempts: 1,
            response_status: 200,
        }
    );

    stop_container(&container.id).unwrap();
    stop_container(&container.id).unwrap();
    assert_eq!(
        observe_container(&container.id, 8080).unwrap(),
        pneuma::domain::runtime::ContainerObservation::not_running(ObservedRuntimeState::Stopped,)
            .unwrap()
    );

    let container_id = container.id.clone();
    container.remove();
    assert_eq!(
        observe_container(&container_id, 8080).unwrap(),
        pneuma::domain::runtime::ContainerObservation::missing()
    );
    let error = start_container(&container_id).unwrap_err();
    assert!(matches!(
        error,
        ControlContainerError::Podman {
            ref stdout,
            ref stderr,
            ..
        } if !stdout.is_empty() || !stderr.is_empty()
    ));

    drop(image);
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "pneuma-local-runtime-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct TestImage {
    reference: String,
}

impl TestImage {
    fn new(reference: String) -> Self {
        Self { reference }
    }
}

impl Drop for TestImage {
    fn drop(&mut self) {
        let _ = Command::new("podman")
            .args(["image", "rm", "--force"])
            .arg(&self.reference)
            .output();
    }
}

struct TestContainer {
    id: String,
}

impl TestContainer {
    fn new(id: String) -> Self {
        Self { id }
    }

    fn remove(mut self) {
        let output = Command::new("podman")
            .args(["container", "rm", "--force"])
            .arg(&self.id)
            .output()
            .unwrap();
        assert_command_succeeded(&output);
        self.id.clear();
    }
}

impl Drop for TestContainer {
    fn drop(&mut self) {
        if self.id.is_empty() {
            return;
        }
        let _ = Command::new("podman")
            .args(["container", "rm", "--force"])
            .arg(&self.id)
            .output();
    }
}

fn inspect(container_id: &str, format: &str) -> String {
    podman(&["inspect", "--format", format, container_id])
        .trim()
        .to_owned()
}

fn podman(arguments: &[&str]) -> String {
    let output = Command::new("podman").args(arguments).output().unwrap();
    assert_command_succeeded(&output);
    String::from_utf8(output.stdout).unwrap()
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
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
