use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::use_cases::application::import_application;

#[test]
fn typed_application_identity_preserves_its_sqlite_text_value() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();

    let persisted_id: String = connection
        .query_row(
            "SELECT id FROM applications WHERE name = ?1",
            [application.name.as_str()],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(persisted_id, application.id.as_str());
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
