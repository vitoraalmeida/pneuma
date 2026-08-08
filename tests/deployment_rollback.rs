use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_rollback::{RollbackError, rollback_deployment};

#[test]
fn rollback_fails_when_no_previous_deployment_exists() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();

    let error = rollback_deployment(&mut connection, &application.id).unwrap_err();

    assert!(matches!(error, RollbackError::NoPreviousDeployment { .. }));
}

#[test]
fn rollback_fails_for_unknown_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let error = rollback_deployment(&mut connection, "non-existent-app").unwrap_err();

    assert!(matches!(error, RollbackError::ApplicationNotFound { .. }));
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
