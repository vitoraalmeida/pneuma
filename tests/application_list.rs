use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::domain::application::{ApplicationName, DesiredRuntimeState};
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::application_list::list_applications;
use pneuma::use_cases::application_lookup::find_application_by_name;

#[test]
fn returns_an_empty_list_for_an_empty_catalog() {
    let connection = database::open(Path::new(":memory:")).unwrap();

    let applications = list_applications(&connection).unwrap();

    assert!(applications.is_empty());
}

#[test]
fn finds_a_core_application_by_name_without_loading_the_catalog() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        Some("https://github.com/vitoraalmeida/vitoralmeida.tech"),
        None,
    )
    .unwrap();

    let application =
        find_application_by_name(&connection, &ApplicationName::new("personal-site").unwrap())
            .unwrap()
            .unwrap();

    assert_eq!(application.name.as_str(), "personal-site");
    assert_eq!(
        application.desired_runtime_state,
        DesiredRuntimeState::Stopped
    );
    assert_eq!(application.manifest_schema_version, 3);
    assert!(
        find_application_by_name(&connection, &ApplicationName::new("missing").unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn returns_registered_applications_ordered_by_name() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        Some("https://github.com/vitoraalmeida/vitoralmeida.tech"),
        Some("pneuma.toml"),
    )
    .unwrap();
    import_application(
        &mut connection,
        &fixture_path("another"),
        None,
        Some("."),
        Some("pneuma.toml"),
    )
    .unwrap();
    connection
        .execute(
            "UPDATE applications SET desired_runtime_state = 'running' WHERE name = 'another-site'",
            [],
        )
        .unwrap();

    let applications = list_applications(&connection).unwrap();

    assert_eq!(applications.len(), 2);
    assert_eq!(applications[0].name.as_str(), "another-site");
    assert_eq!(applications[0].repository.as_deref(), Some("."));
    assert_eq!(applications[0].default_branch.as_deref(), None);
    assert_eq!(
        applications[0].desired_runtime_state,
        DesiredRuntimeState::Running
    );
    assert_eq!(applications[1].name.as_str(), "personal-site");
    assert_eq!(
        applications[1].desired_runtime_state,
        DesiredRuntimeState::Stopped
    );
    assert_eq!(applications[1].manifest_schema_version, 3);
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
    assert_eq!(applications[0].name.as_str(), "legacy-app");
    assert_eq!(applications[0].system_id, None);
    assert_eq!(
        applications[0].desired_runtime_state,
        DesiredRuntimeState::Stopped
    );
    assert_eq!(applications[0].manifest_schema_version, 1);
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
