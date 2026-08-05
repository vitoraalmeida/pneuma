use std::path::{Path, PathBuf};

use pneuma::database;
use pneuma::import_application::{ImportError, import_application};

#[test]
fn imports_and_persists_the_application_specification() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let application = import_application(&mut connection, &fixture_path("valid")).unwrap();

    assert_eq!(application.name, "personal-site");
    assert_eq!(
        application.repository,
        "https://github.com/vitoraalmeida/vitoralmeida.tech"
    );
    assert_eq!(application.default_branch, "main");

    let specification = connection
        .query_row(
            "SELECT
                applications.desired_runtime_state,
                applications.spec_version,
                application_sources.repository_kind,
                application_build_specs.containerfile_path,
                application_build_specs.context_path,
                application_runtime_specs.container_port,
                health_check_specs.path,
                health_check_specs.expected_status,
                exposures.desired_visibility,
                exposures.domain
             FROM applications
             JOIN application_sources
                ON application_sources.application_id = applications.id
             JOIN application_build_specs
                ON application_build_specs.application_id = applications.id
             JOIN application_runtime_specs
                ON application_runtime_specs.application_id = applications.id
             JOIN health_check_specs
                ON health_check_specs.application_id = applications.id
             JOIN exposures
                ON exposures.application_id = applications.id",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(
        specification,
        (
            "stopped".to_owned(),
            1,
            "remote".to_owned(),
            "Containerfile".to_owned(),
            ".".to_owned(),
            8080,
            "/healthz".to_owned(),
            200,
            "public".to_owned(),
            Some("vitoralmeida.tech".to_owned()),
        )
    );
}

#[test]
fn importing_the_same_application_is_idempotent() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = fixture_path("valid");

    let first = import_application(&mut connection, &repository).unwrap();
    let second = import_application(&mut connection, &repository).unwrap();

    let row_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM applications),
                (SELECT COUNT(*) FROM application_sources),
                (SELECT COUNT(*) FROM application_build_specs),
                (SELECT COUNT(*) FROM application_runtime_specs),
                (SELECT COUNT(*) FROM health_check_specs),
                (SELECT COUNT(*) FROM exposures)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(row_counts, (1, 1, 1, 1, 1, 1));
}

#[test]
fn reports_manifest_failures_at_the_import_boundary() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let error = import_application(&mut connection, &fixture_path("missing")).unwrap_err();

    assert!(matches!(error, ImportError::Manifest { .. }));
    assert!(error.to_string().contains("missing/pneuma.toml"));
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
