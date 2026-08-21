use std::process::Command;

use pneuma::adapters::oci_image::{self, ResolveImageDigestError};
use pneuma::domain::git::CommitSha;
use pneuma::domain::release::OciRepository;

fn oci_repository(value: &str) -> OciRepository {
    OciRepository::new(value).unwrap()
}

#[test]
#[ignore = "requires configured rootless Podman"]
fn resolve_image_digest_returns_digest_for_existing_tag() {
    let repository = "localhost:5000/pneuma-oci-test";
    let commit_sha = CommitSha::new(&"a".repeat(40)).unwrap();
    let tagged = format!("{repository}:{}", commit_sha.as_str());

    let build_dir = std::env::temp_dir().join("pneuma-oci-test-build");
    let _ = std::fs::remove_dir_all(&build_dir);
    std::fs::create_dir_all(&build_dir).unwrap();
    std::fs::write(
        build_dir.join("Containerfile"),
        "FROM scratch\nLABEL test=true\n",
    )
    .unwrap();

    let build = Command::new("podman")
        .args(["build", "--tag", &tagged, "--file", "Containerfile", "."])
        .current_dir(&build_dir)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let push = Command::new("podman")
        .args(["push", "--tls-verify=false", &tagged, &tagged])
        .output()
        .unwrap();
    assert!(
        push.status.success(),
        "push failed: {}",
        String::from_utf8_lossy(&push.stderr)
    );

    let reference = oci_image::resolve_image_digest(
        &oci_repository("localhost:5000/pneuma-oci-test"),
        &commit_sha,
    )
    .unwrap();
    assert_eq!(reference.repository(), "localhost:5000/pneuma-oci-test");
    assert!(reference.digest().starts_with("sha256:"));

    let _ = Command::new("podman")
        .args(["image", "rm", "--force", &tagged])
        .output();
    let _ = std::fs::remove_dir_all(&build_dir);
}

#[test]
#[ignore = "requires configured rootless Podman"]
fn resolve_image_digest_fails_for_missing_tag() {
    let commit_sha = CommitSha::new(&"b".repeat(40)).unwrap();

    let error = oci_image::resolve_image_digest(
        &oci_repository("localhost:5000/pneuma-oci-test"),
        &commit_sha,
    )
    .unwrap_err();
    assert!(matches!(error, ResolveImageDigestError::Pull { .. }));
}

#[test]
#[ignore = "requires configured rootless Podman"]
fn resolve_image_digest_fails_for_unreachable_registry() {
    let commit_sha = CommitSha::new(&"c".repeat(40)).unwrap();

    let error = oci_image::resolve_image_digest(
        &oci_repository("localhost:59999/pneuma-oci-test"),
        &commit_sha,
    )
    .unwrap_err();
    assert!(matches!(error, ResolveImageDigestError::Pull { .. }));
}
