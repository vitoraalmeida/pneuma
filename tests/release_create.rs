use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::domain::identity::ApplicationId;
use pneuma::domain::release::OciArtifact;
use pneuma::use_cases::application::import_application;
use pneuma::use_cases::release::{CreateReleaseError, create_release};

#[test]
fn creates_and_reuses_a_release_from_one_validated_artifact() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let artifact = OciArtifact::new(
        "ghcr.io/vitoraalmeida/vitoralmeida.tech",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap();

    let first = create_release(&mut connection, &application.id, &artifact).unwrap();
    let second = create_release(&mut connection, &application.id, &artifact).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.artifact, artifact);
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn rejects_invalid_artifact_identity_before_release_creation() {
    assert!(OciArtifact::parse("registry.example/app:latest").is_err());
    assert!(OciArtifact::new("registry.example/app", "sha256:not-a-digest").is_err());
}

#[test]
fn reports_the_actual_missing_application_identifier() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let artifact = OciArtifact::new(
        "registry.example/app",
        &format!("sha256:{}", "b".repeat(64)),
    )
    .unwrap();

    let application_id = ApplicationId::new("22222222222222222222222222222222").unwrap();
    let error = create_release(&mut connection, &application_id, &artifact).unwrap_err();

    assert!(matches!(
        error,
        CreateReleaseError::ApplicationNotFound { application_id }
            if application_id == "22222222222222222222222222222222"
    ));
}

#[test]
fn persists_only_the_canonical_artifact_reference() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let artifact = OciArtifact::new(
        "ghcr.io/vitoraalmeida/vitoralmeida.tech",
        &format!("sha256:{}", "c".repeat(64)),
    )
    .unwrap();
    create_release(&mut connection, &application.id, &artifact).unwrap();

    // Repository and digest are derived by parsing the one canonical column;
    // redundant artifact columns are unrepresentable in the current schema.
    let redundant_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('releases')
             WHERE name IN ('image_repository', 'image_digest', 'source_revision')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(redundant_columns, 0);
}
