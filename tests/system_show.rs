use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::domain::application::DesiredRuntimeState;
use pneuma::domain::system::SystemName;
use pneuma::use_cases::application::import_application;
use pneuma::use_cases::system::show_system;

#[test]
fn returns_the_application_runtime_intent() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://github.com/vitoraalmeida/vitoralmeida.tech",
        None,
    )
    .unwrap();
    connection
        .execute(
            "UPDATE applications SET desired_runtime_state = 'running' WHERE name = 'personal-site'",
            [],
        )
        .unwrap();

    let system_name = SystemName::new("personal-website").unwrap();
    let details = show_system(&connection, &system_name).unwrap();

    assert_eq!(details.applications.len(), 1);
    assert_eq!(
        details.applications[0].desired_runtime_state,
        DesiredRuntimeState::Running
    );
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
