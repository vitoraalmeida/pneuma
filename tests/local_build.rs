use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::local_build::{BuildImageError, build_image};

#[test]
fn rejects_a_missing_build_path_before_running_podman() {
    let temporary_directory = TemporaryDirectory::new();

    let error = build_image(
        &temporary_directory.path,
        "personal-site",
        "e48c715",
        Path::new("Containerfile"),
        Path::new("."),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        BuildImageError::ResolvePath {
            field: "containerfile",
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn rejects_a_build_path_that_escapes_the_checkout() {
    use std::os::unix::fs::symlink;

    let temporary_directory = TemporaryDirectory::new();
    let checkout = temporary_directory.path.join("checkout");
    let outside = temporary_directory.path.join("outside");
    fs::create_dir(&checkout).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(checkout.join("Containerfile"), "FROM scratch\n").unwrap();
    symlink(&outside, checkout.join("context")).unwrap();

    let error = build_image(
        &checkout,
        "personal-site",
        "e48c715",
        Path::new("Containerfile"),
        Path::new("context"),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        BuildImageError::OutsideCheckout {
            field: "context",
            ..
        }
    ));
}

#[test]
#[ignore = "requires configured rootless Podman"]
fn builds_an_image_and_preserves_failure_diagnostics() {
    let temporary_directory = TemporaryDirectory::new();
    let checkout = &temporary_directory.path;
    fs::write(
        checkout.join("Containerfile"),
        "FROM scratch\nCOPY artifact.txt /artifact.txt\n",
    )
    .unwrap();
    fs::write(checkout.join("artifact.txt"), "artifact contents").unwrap();
    let commit_sha = format!("{:040x}", unique_suffix());

    let built = build_image(
        checkout,
        "pneuma-build-test",
        &commit_sha,
        Path::new("Containerfile"),
        Path::new("."),
    )
    .unwrap();
    let image = TestImage::new(built.reference.clone());

    let inspect = Command::new("podman")
        .args(["image", "inspect"])
        .arg(&image.reference)
        .output()
        .unwrap();
    assert_command_succeeded(&inspect);
    assert_eq!(
        built.reference,
        format!("localhost/pneuma/pneuma-build-test:{commit_sha}")
    );
    assert!(!built.stdout.is_empty() || !built.stderr.is_empty());

    fs::write(checkout.join("Containerfile"), "NOT_A_CONTAINERFILE\n").unwrap();
    let error = build_image(
        checkout,
        "pneuma-build-test",
        &commit_sha,
        Path::new("Containerfile"),
        Path::new("."),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        BuildImageError::Build {
            ref stdout,
            ref stderr,
            ..
        } if !stdout.is_empty() || !stderr.is_empty()
    ));
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "pneuma-local-build-{}-{}",
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
