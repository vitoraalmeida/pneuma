use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::git_source::{
    CreateCheckoutError, ResolveCommitError, create_checkout, resolve_commit,
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

        assert_eq!(commit, expected_commit);
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
