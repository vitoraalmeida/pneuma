use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::domain::identity::ApplicationId;
use pneuma::use_cases::application::import_application;
use pneuma::use_cases::deployment::{RollbackError, rollback_deployment};

#[test]
fn rollback_fails_when_no_previous_deployment_exists() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();

    let error = rollback_deployment(&mut connection, &application.id, None).unwrap_err();

    assert!(matches!(error, RollbackError::NoPreviousDeployment { .. }));
}

#[test]
fn rollback_fails_for_unknown_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let application_id = ApplicationId::new("22222222222222222222222222222222").unwrap();
    let error = rollback_deployment(&mut connection, &application_id, None).unwrap_err();

    assert!(matches!(error, RollbackError::ApplicationNotFound { .. }));
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
