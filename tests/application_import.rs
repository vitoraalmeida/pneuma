use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::use_cases::application_import::{ImportError, import_application};
use pneuma::use_cases::application_list::list_applications;

#[test]
fn imports_and_persists_the_application_specification() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        Some("https://github.com/vitoraalmeida/vitoralmeida.tech"),
        Some("deploy/staging/pneuma.toml"),
    )
    .unwrap();

    assert_eq!(application.name, "personal-site");
    assert_eq!(
        application.repository.as_deref(),
        Some("https://github.com/vitoraalmeida/vitoralmeida.tech")
    );
    assert_eq!(application.default_branch.as_deref(), None);

    let specification = connection
        .query_row(
            "SELECT
                applications.desired_runtime_state,
                applications.spec_version,
                application_sources.repository_kind,
                application_sources.manifest_path,
                application_runtime_specs.container_port,
                health_check_specs.path,
                health_check_specs.expected_status,
                exposures.desired_visibility,
                exposures.domain,
                application_delivery_specs.delivery_type,
                application_delivery_specs.image_repository
             FROM applications
             JOIN application_sources
                ON application_sources.application_id = applications.id
             JOIN application_runtime_specs
                ON application_runtime_specs.application_id = applications.id
             JOIN health_check_specs
                ON health_check_specs.application_id = applications.id
             JOIN exposures
                ON exposures.application_id = applications.id
             JOIN application_delivery_specs
                ON application_delivery_specs.application_id = applications.id",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(
        specification,
        (
            "stopped".to_owned(),
            3,
            "remote".to_owned(),
            "deploy/staging/pneuma.toml".to_owned(),
            8080,
            "/healthz".to_owned(),
            200,
            "public".to_owned(),
            Some("vitoralmeida.tech".to_owned()),
            "oci".to_owned(),
            "ghcr.io/vitoraalmeida/vitoralmeida.tech".to_owned(),
        )
    );
}

#[test]
fn importing_the_same_application_is_idempotent() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = fixture_path("valid");

    let repository_url = "https://github.com/vitoraalmeida/vitoralmeida.tech";
    let first = import_application(
        &mut connection,
        &repository,
        None,
        Some(repository_url),
        Some("deploy/staging/pneuma.toml"),
    )
    .unwrap();
    let second = import_application(
        &mut connection,
        &repository,
        None,
        Some(repository_url),
        Some("deploy/staging/pneuma.toml"),
    )
    .unwrap();

    let row_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM applications),
                (SELECT COUNT(*) FROM application_sources),
                (SELECT COUNT(*) FROM application_runtime_specs),
                (SELECT COUNT(*) FROM health_check_specs),
                (SELECT COUNT(*) FROM exposures),
                (SELECT COUNT(*) FROM application_delivery_specs)",
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
fn reports_manifest_failures_without_changing_the_catalog() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let error = import_application(&mut connection, &fixture_path("missing"), None, None, None)
        .unwrap_err();
    let applications = list_applications(&connection).unwrap();

    assert!(matches!(error, ImportError::Manifest { .. }));
    assert!(error.to_string().contains("missing/pneuma.toml"));
    assert!(applications.is_empty());
}

#[test]
fn persists_a_local_repository_kind() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    import_application(
        &mut connection,
        &fixture_path("another"),
        None,
        Some("."),
        Some("pneuma.toml"),
    )
    .unwrap();

    let repository_kind: String = connection
        .query_row(
            "SELECT repository_kind FROM application_sources",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(repository_kind, "local");
}

#[test]
fn classifies_ssh_git_urls_as_remote() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        Some("git@github.com:vitoraalmeida/vitoralmeida.tech.git"),
        Some("pneuma.toml"),
    )
    .unwrap();

    let repository_kind: String = connection
        .query_row(
            "SELECT repository_kind FROM application_sources",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(repository_kind, "remote");
}

#[test]
fn persists_delivery_without_source_or_build_specs() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    import_application(&mut connection, &fixture_path("oci-only"), None, None, None).unwrap();

    let delivery: (String, String) = connection
        .query_row(
            "SELECT delivery_type, image_repository FROM application_delivery_specs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        delivery,
        ("oci".to_owned(), "registry.example/team/service".to_owned())
    );
    let source_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM application_sources", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(source_count, 0);
}

#[test]
fn requires_system_from_manifest_or_cli() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    assert!(application.system_id.is_some());
}

#[test]
fn cli_system_overrides_manifest() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        Some("cli-system"),
        None,
        None,
    )
    .unwrap();

    let system_name: String = connection
        .query_row(
            "SELECT systems.name FROM systems
             JOIN applications ON applications.system_id = systems.id
             WHERE applications.id = ?1",
            [&application.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(system_name, "cli-system");
}

#[test]
fn reimporting_a_remote_oci_import_keeps_a_single_spec_row() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = fixture_path("oci-only");
    let repository_url = "https://git.example.com/team/service";

    let first = import_application(
        &mut connection,
        &repository,
        None,
        Some(repository_url),
        None,
    )
    .unwrap();
    let second = import_application(
        &mut connection,
        &repository,
        None,
        Some(repository_url),
        None,
    )
    .unwrap();

    let row_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM applications),
                (SELECT COUNT(*) FROM application_delivery_specs),
                (SELECT COUNT(*) FROM application_sources)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row_counts, (1, 1, 1));
    assert_eq!(first, second);
}

#[test]
fn reimport_preserves_the_active_deployment_of_a_deployed_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = fixture_path("valid");
    let repository_url = "https://github.com/vitoraalmeida/vitoralmeida.tech";

    import_application(
        &mut connection,
        &repository,
        None,
        Some(repository_url),
        None,
    )
    .unwrap();

    let application_id: String = connection
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    connection
        .execute(
            "INSERT INTO releases (id, application_id, image_repository, image_digest, created_at)
             VALUES ('release-1', ?1, 'ghcr.io/vitoraalmeida/vitoralmeida.tech',
                     'sha256:deadbeef', CURRENT_TIMESTAMP)",
            [&application_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO deployments (id, application_id, release_id, type, status,
                                      created_at, updated_at)
             VALUES ('deployment-1', ?1, 'release-1', 'deploy', 'succeeded',
                     CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            [&application_id],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE applications SET active_deployment_id = 'deployment-1' WHERE id = ?1",
            [&application_id],
        )
        .unwrap();

    let reimported = import_application(
        &mut connection,
        &repository,
        None,
        Some(repository_url),
        None,
    )
    .unwrap();

    assert_eq!(reimported.id, application_id);
    assert_eq!(
        reimported.active_deployment_id.as_deref(),
        Some("deployment-1")
    );
    let (source_url, source_count): (Option<String>, i64) = connection
        .query_row(
            "SELECT
                (SELECT repository_url FROM application_sources),
                (SELECT COUNT(*) FROM application_sources)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(source_url.as_deref(), Some(repository_url));
    assert_eq!(source_count, 1);
}

#[test]
fn a_mid_aggregate_persistence_failure_rolls_back_everything() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_runtime_spec_insert
             BEFORE INSERT ON application_runtime_specs
             BEGIN
                 SELECT RAISE(ABORT, 'intentional failure');
             END",
        )
        .unwrap();

    let error = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        Some("https://github.com/vitoraalmeida/vitoralmeida.tech"),
        None,
    )
    .unwrap_err();

    assert!(matches!(error, ImportError::Persistence { .. }));
    let row_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM systems),
                (SELECT COUNT(*) FROM applications),
                (SELECT COUNT(*) FROM application_delivery_specs),
                (SELECT COUNT(*) FROM application_sources),
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
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row_counts, (0, 0, 0, 0, 0, 0, 0));
}

#[test]
fn reimport_is_create_only_when_arguments_diverge() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = fixture_path("valid");
    let original_url = "https://github.com/vitoraalmeida/vitoralmeida.tech";

    let original = import_application(
        &mut connection,
        &repository,
        Some("system-a"),
        Some(original_url),
        None,
    )
    .unwrap();

    let divergent = import_application(
        &mut connection,
        &repository,
        Some("system-b"),
        Some("https://git.example.com/different/repository"),
        None,
    )
    .unwrap();

    assert_eq!(divergent.id, original.id);
    assert_eq!(divergent.system_id, original.system_id);
    assert_eq!(divergent.repository.as_deref(), Some(original_url));

    let (system_count, system_name): (i64, String) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM systems),
                (SELECT name FROM systems)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(system_count, 1);
    assert_eq!(system_name, "system-a");
    let source_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM application_sources", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(source_count, 1);
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
