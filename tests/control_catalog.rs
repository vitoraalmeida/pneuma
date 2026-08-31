use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor, HostConfiguration};
use pneuma::use_cases::application::ApplicationLookupError;

// Executes the catalog and query command families through the library boundary
// without Clap, terminal output, or exit codes, receiving only typed results.
#[test]
fn imports_and_lists_applications_through_the_boundary() {
    let root = temporary_root("catalog-import");
    let workspace = root.join("checkouts");
    let executor = ControlExecutor::new(HostConfiguration::new(
        root.join("pneuma.sqlite3"),
        workspace.clone(),
    ));
    let repository = TestRepository::new(&root, "valid");
    let url = format!("file://{}", repository.path.display());

    let imported = executor
        .execute(Command::ImportApplication {
            repository: url.clone(),
            system_name: None,
            manifest_path: None,
        })
        .unwrap();
    let CommandResult::ApplicationImported(application) = imported else {
        panic!("ImportApplication must yield ApplicationImported");
    };
    assert_eq!(application.name.as_str(), "personal-site");
    assert!(application.active_deployment_id.is_none());
    assert!(
        workspace.join("imports").is_dir(),
        "the import workspace must come from the host configuration"
    );

    let repeated = executor
        .execute(Command::ImportApplication {
            repository: url,
            system_name: None,
            manifest_path: None,
        })
        .unwrap();
    let CommandResult::ApplicationImported(repeated) = repeated else {
        panic!("ImportApplication must yield ApplicationImported");
    };
    assert_eq!(repeated.id, application.id, "import is idempotent by name");

    let listed = executor.execute(Command::ListApplications).unwrap();
    let CommandResult::Applications(entries) = listed else {
        panic!("ListApplications must yield Applications");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].summary.id, application.id);
    assert_eq!(entries[0].summary.name, application.name);
    assert!(!entries[0].deployed, "a fresh import has no deployment");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deployment_history_is_listed_per_application_through_the_boundary() {
    let root = temporary_root("catalog-history");
    let executor = ControlExecutor::new(HostConfiguration::new(
        root.join("pneuma.sqlite3"),
        root.join("checkouts"),
    ));
    let repository = TestRepository::new(&root, "valid");
    let url = format!("file://{}", repository.path.display());
    executor
        .execute(Command::ImportApplication {
            repository: url,
            system_name: None,
            manifest_path: None,
        })
        .unwrap();

    let history = executor
        .execute(Command::ListDeployments {
            application_name: "personal-site".to_owned(),
        })
        .unwrap();
    let CommandResult::ApplicationDeployments {
        application_name,
        deployments,
    } = history
    else {
        panic!("ListDeployments must yield ApplicationDeployments");
    };
    assert_eq!(application_name.as_str(), "personal-site");
    assert!(deployments.is_empty(), "a fresh import has no deployments");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn listing_deployments_of_a_missing_application_is_a_typed_not_found_error() {
    let root = temporary_root("catalog-missing");
    let executor = ControlExecutor::new(HostConfiguration::new(
        root.join("pneuma.sqlite3"),
        root.join("checkouts"),
    ));

    let error = executor
        .execute(Command::ListDeployments {
            application_name: "missing".to_owned(),
        })
        .unwrap_err();

    assert!(matches!(
        error,
        ControlError::ApplicationLookup {
            source: ApplicationLookupError::NotFound { .. }
        }
    ));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_local_import_path_is_rejected_without_persisting_an_application() {
    let root = temporary_root("catalog-local-path");
    let executor = ControlExecutor::new(HostConfiguration::new(
        root.join("pneuma.sqlite3"),
        root.join("checkouts"),
    ));

    let error = executor
        .execute(Command::ImportApplication {
            repository: root.display().to_string(),
            system_name: None,
            manifest_path: None,
        })
        .unwrap_err();
    assert!(matches!(error, ControlError::Import { .. }));

    let listed = executor.execute(Command::ListApplications).unwrap();
    let CommandResult::Applications(entries) = listed else {
        panic!("ListApplications must yield Applications");
    };
    assert!(entries.is_empty(), "a rejected import must persist nothing");

    fs::remove_dir_all(root).unwrap();
}

fn temporary_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pneuma-control-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

// Creates a real Git repository seeded with one manifest fixture so the
// boundary's import command clones through the normal remote-source path.
struct TestRepository {
    path: PathBuf,
}

impl TestRepository {
    fn new(root: &Path, fixture: &str) -> Self {
        let path = root.join("remote");
        fs::create_dir(&path).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(fixture)
                .join("pneuma.toml"),
            path.join("pneuma.toml"),
        )
        .unwrap();
        run_git(&path, &["init", "--quiet", "--initial-branch=main"]);
        run_git(&path, &["add", "pneuma.toml"]);
        run_git(
            &path,
            &[
                "-c",
                "user.name=Pneuma Tests",
                "-c",
                "user.email=pneuma@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "initial commit",
            ],
        );
        Self { path }
    }
}

fn run_git(directory: &Path, arguments: &[&str]) {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert_git_succeeded(&output);
}

fn assert_git_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
