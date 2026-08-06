use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::local_build::build_image;
use pneuma::local_runtime::{CreateContainerError, create_container};

#[test]
#[ignore = "requires configured rootless Podman"]
fn creates_a_stopped_loopback_only_candidate_and_preserves_failure_diagnostics() {
    assert_eq!(
        podman(&["info", "--format", "{{.Host.Security.Rootless}}"]).trim(),
        "true"
    );

    let temporary_directory = TemporaryDirectory::new();
    fs::write(
        temporary_directory.path.join("Containerfile"),
        "FROM scratch\nCMD [\"/bin/true\"]\n",
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

    drop(container);
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
            .args(["image", "remove", "--force"])
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
}

impl Drop for TestContainer {
    fn drop(&mut self) {
        let _ = Command::new("podman")
            .args(["container", "remove", "--force"])
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
