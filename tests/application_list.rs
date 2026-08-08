use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::application_list::list_applications;

#[test]
fn returns_an_empty_list_for_an_empty_catalog() {
    let connection = database::open(Path::new(":memory:")).unwrap();

    let applications = list_applications(&connection).unwrap();

    assert!(applications.is_empty());
}

#[test]
fn returns_registered_applications_ordered_by_name() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    import_application(&mut connection, &fixture_path("valid"), None).unwrap();
    import_application(&mut connection, &fixture_path("another"), None).unwrap();

    let applications = list_applications(&connection).unwrap();

    assert_eq!(applications.len(), 2);
    assert_eq!(applications[0].name, "another-site");
    assert_eq!(applications[0].repository.as_deref(), Some("."));
    assert_eq!(applications[0].default_branch.as_deref(), Some("trunk"));
    assert_eq!(applications[1].name, "personal-site");
}

#[test]
fn lists_legacy_applications_without_a_system() {
    let connection = database::open(Path::new(":memory:")).unwrap();
    connection
        .execute_batch(
            "INSERT INTO applications (
                id, name, desired_runtime_state, spec_version, created_at, updated_at
             ) VALUES ('legacy-id', 'legacy-app', 'stopped', 1, 'now', 'now')",
        )
        .unwrap();

    let applications = list_applications(&connection).unwrap();

    assert_eq!(applications.len(), 1);
    assert_eq!(applications[0].name, "legacy-app");
    assert_eq!(applications[0].system_id, None);
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
