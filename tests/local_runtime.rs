use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::health_check::{HealthCheckResult, check_internal_health};
use pneuma::adapters::local_build::build_image;
use pneuma::adapters::local_runtime::{
    ControlContainerError, CreateContainerError, ObservedRuntimeState, create_container,
    observe_container, start_container, stop_container,
};

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
    let built = build_image(
        &temporary_directory.path,
        "pneuma-runtime-test",
        &commit_sha,
        Path::new("Containerfile"),
        Path::new("."),
    )
    .unwrap();
    let image = TestImage::new(built.reference.clone());

    let created =
        create_container(&built.reference, "pneuma-runtime-test", &commit_sha, 8080).unwrap();
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
            "{{ index .Config.Labels \"io.pneuma.role\" }}"
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

    let error =
        create_container(&built.reference, "pneuma-runtime-test", &commit_sha, 8080).unwrap_err();
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
        pneuma::adapters::local_runtime::ContainerObservation {
            state: ObservedRuntimeState::Created,
            endpoint: None,
        }
    );

    start_container(&container.id).unwrap();
    start_container(&container.id).unwrap();
    let running = observe_container(&container.id, 8080).unwrap();
    assert_eq!(running.state, ObservedRuntimeState::Running);
    let endpoint = running
        .endpoint
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
        pneuma::adapters::local_runtime::ContainerObservation {
            state: ObservedRuntimeState::Stopped,
            endpoint: None,
        }
    );

    let container_id = container.id.clone();
    container.remove();
    assert_eq!(
        observe_container(&container_id, 8080).unwrap(),
        pneuma::adapters::local_runtime::ContainerObservation {
            state: ObservedRuntimeState::Missing,
            endpoint: None,
        }
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
