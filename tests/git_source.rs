use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::git_source::{
    CloneRepositoryError, CommitSha, CreateCheckoutError, ResolveBranchError, ResolveCommitError,
    cleanup_checkout, clone_repository, create_checkout, ensure_checkout, is_remote_repository,
    resolve_branch, resolve_commit,
};

#[test]
fn resolves_branches_tags_and_abbreviated_shas_without_changing_the_repository() {
    let repository = TestRepository::new();
    let expected_commit = repository.head_commit();
    repository.create_annotated_tag("v1");
    let abbreviated_commit = &expected_commit[..7];
    let status_before = repository.status();

    for revision in ["main", "v1", abbreviated_commit] {
        let commit = resolve_commit(&repository.path, revision).unwrap();

        assert_eq!(commit.as_str(), expected_commit);
    }

    assert_eq!(repository.status(), status_before);
}

#[test]
fn rejects_a_missing_revision() {
    let repository = TestRepository::new();

    let error = resolve_commit(&repository.path, "missing").unwrap_err();

    assert!(matches!(error, ResolveCommitError::Resolve { .. }));
    assert!(error.to_string().contains("missing"));
    assert!(
        error
            .to_string()
            .contains(repository.path.to_string_lossy().as_ref())
    );
}

#[test]
fn rejects_an_object_that_is_not_a_commit() {
    let repository = TestRepository::new();
    let tree = repository.git(&["rev-parse", "HEAD^{tree}"]);

    let error = resolve_commit(&repository.path, tree.trim()).unwrap_err();

    assert!(matches!(error, ResolveCommitError::Resolve { .. }));
}

#[test]
fn creates_independent_checkouts_for_two_commits() {
    let repository = TestRepository::new();
    let first_commit = repository.head_commit();
    let second_commit = repository.commit_file("second contents", "second commit");
    let first_checkout = repository.temporary_root.join("first-checkout");
    let second_checkout = repository.temporary_root.join("second-checkout");

    create_checkout(&repository.path, &first_commit, &first_checkout).unwrap();
    create_checkout(&repository.path, &second_commit, &second_checkout).unwrap();

    assert_eq!(
        fs::read_to_string(first_checkout.join("site.txt")).unwrap(),
        "initial contents"
    );
    assert_eq!(
        fs::read_to_string(second_checkout.join("site.txt")).unwrap(),
        "second contents"
    );

    fs::write(first_checkout.join("site.txt"), "changed checkout").unwrap();
    assert_eq!(
        fs::read_to_string(second_checkout.join("site.txt")).unwrap(),
        "second contents"
    );
    assert_eq!(
        fs::read_to_string(repository.path.join("site.txt")).unwrap(),
        "second contents"
    );
}

#[test]
fn rejects_an_existing_checkout_destination() {
    let repository = TestRepository::new();
    let destination = repository.temporary_root.join("existing");
    fs::create_dir(&destination).unwrap();

    let error =
        create_checkout(&repository.path, &repository.head_commit(), &destination).unwrap_err();

    assert!(matches!(
        error,
        CreateCheckoutError::DestinationExists { .. }
    ));
}

#[test]
fn reuses_a_clean_checkout_at_the_same_commit() {
    let repository = TestRepository::new();
    let commit = repository.head_commit();
    let destination = repository.temporary_root.join("checkout");
    create_checkout(&repository.path, &commit, &destination).unwrap();

    ensure_checkout(&repository.path, &commit, &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("site.txt")).unwrap(),
        "initial contents"
    );
}

#[test]
fn replaces_a_dirty_checkout_at_the_same_commit() {
    let repository = TestRepository::new();
    let commit = repository.head_commit();
    let destination = repository.temporary_root.join("checkout");
    create_checkout(&repository.path, &commit, &destination).unwrap();
    fs::write(destination.join("site.txt"), "local changes").unwrap();

    ensure_checkout(&repository.path, &commit, &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("site.txt")).unwrap(),
        "initial contents"
    );
}

#[test]
fn replaces_a_checkout_at_a_different_commit() {
    let repository = TestRepository::new();
    let first_commit = repository.head_commit();
    let second_commit = repository.commit_file("second contents", "second commit");
    let destination = repository.temporary_root.join("checkout");
    create_checkout(&repository.path, &first_commit, &destination).unwrap();

    ensure_checkout(&repository.path, &second_commit, &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("site.txt")).unwrap(),
        "second contents"
    );
}

#[test]
fn classifies_remote_and_local_repositories() {
    assert!(is_remote_repository(
        "https://github.com/vitoraalmeida/vitoralmeida.tech.git"
    ));
    assert!(is_remote_repository(
        "git@github.com:vitoraalmeida/vitoralmeida.tech.git"
    ));
    assert!(!is_remote_repository("/srv/checkouts/vitoralmeida.tech"));
    assert!(!is_remote_repository("."));
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

    fn status(&self) -> String {
        self.git(&["status", "--porcelain"])
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
