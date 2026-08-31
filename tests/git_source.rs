use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::git_source::{
    CloneRepositoryError, ResolveBranchError, cleanup_checkout, clone_repository, resolve_branch,
};
use pneuma::domain::git::{CommitSha, is_remote_git_location};

#[test]
fn classifies_remote_and_local_repositories() {
    assert!(is_remote_git_location(
        "https://github.com/vitoraalmeida/vitoralmeida.tech.git"
    ));
    assert!(is_remote_git_location(
        "git@github.com:vitoraalmeida/vitoralmeida.tech.git"
    ));
    assert!(!is_remote_git_location("/srv/checkouts/vitoralmeida.tech"));
    assert!(!is_remote_git_location("."));
}

#[test]
fn clones_a_repository_by_url_and_cleans_up_the_checkout() {
    let repository = TestRepository::new();
    let url = format!("file://{}", repository.path.display());
    let destination = repository.temporary_root.join("clone");

    clone_repository(&url, &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("site.txt")).unwrap(),
        "initial contents"
    );

    cleanup_checkout(&destination).unwrap();
    assert!(!destination.exists());

    // A repeated cleanup after an earlier removal must still succeed so
    // abandoned-import recovery can be retried safely.
    cleanup_checkout(&destination).unwrap();
}

#[test]
fn rejects_an_unreachable_repository_url() {
    let repository = TestRepository::new();
    let destination = repository.temporary_root.join("clone");
    let url = format!("file://{}/missing", repository.temporary_root.display());

    let error = clone_repository(&url, &destination).unwrap_err();

    assert!(matches!(error, CloneRepositoryError::Git { .. }));
    assert!(!destination.exists());
}

#[test]
fn rejects_an_existing_clone_destination() {
    let repository = TestRepository::new();
    let destination = repository.temporary_root.join("existing");
    fs::create_dir(&destination).unwrap();

    let error = clone_repository("https://example.invalid/repo.git", &destination).unwrap_err();

    assert!(matches!(
        error,
        CloneRepositoryError::DestinationExists { .. }
    ));
}

#[test]
fn resolves_a_branch_to_a_commit_sha() {
    let repository = TestRepository::new();
    let branch_commit = repository.commit_file("staging contents", "staging commit");
    repository.create_branch("staging");

    let sha = resolve_branch(&url_for(&repository), "staging").unwrap();

    assert_eq!(sha.as_str(), branch_commit);
}

#[test]
fn resolves_the_default_branch_to_a_commit_sha() {
    let repository = TestRepository::new();
    let expected_commit = repository.head_commit();

    let sha = resolve_branch(&url_for(&repository), "main").unwrap();

    assert_eq!(sha.as_str(), expected_commit);
}

#[test]
fn resolves_an_annotated_tag_to_a_commit_sha() {
    let repository = TestRepository::new();
    let expected_commit = repository.head_commit();
    repository.create_annotated_tag("v1");

    let sha = resolve_branch(&url_for(&repository), "v1").unwrap();

    assert_eq!(sha.as_str(), expected_commit);
}

#[test]
fn resolves_a_lightweight_tag_to_a_commit_sha() {
    let repository = TestRepository::new();
    let expected_commit = repository.head_commit();
    repository.create_lightweight_tag("v1");

    let sha = resolve_branch(&url_for(&repository), "v1").unwrap();

    assert_eq!(sha.as_str(), expected_commit);
}

#[test]
fn rejects_a_missing_branch() {
    let repository = TestRepository::new();

    let error = resolve_branch(&url_for(&repository), "missing").unwrap_err();

    assert!(matches!(error, ResolveBranchError::BranchNotFound { .. }));
    assert!(error.to_string().contains("missing"));
}

#[test]
fn rejects_a_missing_repository() {
    let repository = TestRepository::new();
    let url = format!("file://{}/missing", repository.temporary_root.display());

    let error = resolve_branch(&url, "main").unwrap_err();

    assert!(matches!(
        error,
        ResolveBranchError::RepositoryNotFound { .. }
    ));
}

#[test]
fn commit_sha_rejects_an_invalid_identifier() {
    for invalid in ["", "abc", "G".repeat(40).as_str(), &"a".repeat(39)] {
        assert!(
            CommitSha::new(invalid).is_err(),
            "expected rejection: {invalid}"
        );
    }
}

#[test]
fn commit_sha_accepts_a_full_hexadecimal_sha() {
    let sha = "a".repeat(40);

    let parsed = CommitSha::new(&sha).unwrap();

    assert_eq!(parsed.as_str(), sha);
}

struct TestRepository {
    temporary_root: PathBuf,
    path: PathBuf,
}

impl TestRepository {
    fn new() -> Self {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temporary_root = env::temp_dir().join(format!(
            "pneuma-git-source-{}-{unique_suffix}",
            std::process::id()
        ));
        fs::create_dir(&temporary_root).unwrap();
        let path = temporary_root.join("repository");

        let output = Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .arg(&path)
            .output()
            .unwrap();
        assert_git_succeeded(&output);

        fs::write(path.join("site.txt"), "initial contents").unwrap();
        let repository = Self {
            temporary_root,
            path,
        };
        repository.git(&["add", "site.txt"]);
        repository.git(&[
            "-c",
            "user.name=Pneuma Tests",
            "-c",
            "user.email=pneuma@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "initial commit",
        ]);
        repository
    }

    fn git(&self, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(arguments)
            .output()
            .unwrap();
        assert_git_succeeded(&output);
        String::from_utf8(output.stdout).unwrap()
    }

    fn commit_file(&self, contents: &str, message: &str) -> String {
        fs::write(self.path.join("site.txt"), contents).unwrap();
        self.git(&["add", "site.txt"]);
        self.git(&[
            "-c",
            "user.name=Pneuma Tests",
            "-c",
            "user.email=pneuma@example.invalid",
            "commit",
            "--quiet",
            "-m",
            message,
        ]);
        self.head_commit()
    }

    fn head_commit(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn create_annotated_tag(&self, tag: &str) {
        self.git(&[
            "-c",
            "user.name=Pneuma Tests",
            "-c",
            "user.email=pneuma@example.invalid",
            "tag",
            "--annotate",
            tag,
            "--message",
            tag,
        ]);
    }

    fn create_lightweight_tag(&self, tag: &str) {
        self.git(&["tag", tag]);
    }

    fn create_branch(&self, branch: &str) {
        self.git(&["branch", branch]);
    }
}

impl Drop for TestRepository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temporary_root);
    }
}

fn assert_git_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn url_for(repository: &TestRepository) -> String {
    format!("file://{}", repository.path.display())
}
