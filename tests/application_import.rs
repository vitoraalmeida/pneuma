use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::domain::application::DesiredRuntimeState;
use pneuma::domain::system::SystemName;
use pneuma::use_cases::application::list_applications;
use pneuma::use_cases::application::{ImportError, import_application};

#[test]
fn imports_and_persists_the_application_specification() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://github.com/vitoraalmeida/vitoralmeida.tech",
        Some("deploy/staging/pneuma.toml"),
    )
    .unwrap();

    assert_eq!(application.name.as_str(), "personal-site");
    assert_eq!(
        application.repository.as_str(),
        "https://github.com/vitoraalmeida/vitoralmeida.tech"
    );
    assert_eq!(application.default_branch.as_deref(), None);
    assert_eq!(
        application.desired_runtime_state,
        DesiredRuntimeState::Stopped
    );
    let specification = connection
        .query_row(
            "SELECT
                applications.repository_url,
                applications.manifest_path,
                applications.container_port,
                applications.health_check_path,
                applications.health_check_expected_status,
                applications.image_repository,
                exposures.desired_visibility,
                exposures.domain
             FROM applications
             JOIN exposures ON exposures.application_id = applications.id",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(
        specification,
        (
            "https://github.com/vitoraalmeida/vitoralmeida.tech".to_owned(),
            "deploy/staging/pneuma.toml".to_owned(),
            8080,
            "/healthz".to_owned(),
            200,
            "ghcr.io/vitoraalmeida/vitoralmeida.tech".to_owned(),
            "public".to_owned(),
            Some("vitoralmeida.tech".to_owned()),
        )
    );
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
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
        repository_url,
        Some("deploy/staging/pneuma.toml"),
    )
    .unwrap();
    let second = import_application(
        &mut connection,
        &repository,
        None,
        repository_url,
        Some("deploy/staging/pneuma.toml"),
    )
    .unwrap();

    let row_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM applications),
                (SELECT COUNT(*) FROM exposures),
                (SELECT COUNT(*) FROM systems)",
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

    assert_eq!(first, second);
    assert_eq!(row_counts, (1, 1, 1));
}

#[test]
fn reports_manifest_failures_without_changing_the_catalog() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let error = import_application(
        &mut connection,
        &fixture_path("missing"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap_err();
    let applications = list_applications(&connection).unwrap();

    assert!(matches!(error, ImportError::Manifest { .. }));
    assert!(error.to_string().contains("missing/pneuma.toml"));
    assert!(applications.is_empty());
}

#[test]
fn rejects_local_repository_paths() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let error = import_application(
        &mut connection,
        &fixture_path("another"),
        None,
        ".",
        Some("pneuma.toml"),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("remote Git URL"),
        "local paths are not a supported source: {error}"
    );
    let applications = list_applications(&connection).unwrap();
    assert!(applications.is_empty());
}

#[test]
fn classifies_ssh_git_urls_as_remote() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "git@github.com:vitoraalmeida/vitoralmeida.tech.git",
        Some("pneuma.toml"),
    )
    .unwrap();

    let repository_url: String = connection
        .query_row("SELECT repository_url FROM applications", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        repository_url,
        "git@github.com:vitoraalmeida/vitoralmeida.tech.git"
    );
}

#[test]
fn requires_system_from_manifest_or_cli() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    // Every imported Application carries exactly one required System identity.
    assert_eq!(application.system_id.to_string().len(), 32);
}

#[test]
fn cli_system_overrides_manifest() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let system_name = SystemName::new("cli-system").unwrap();

    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        Some(&system_name),
        "https://example.test/app.git",
        None,
    )
    .unwrap();

    let system_name: String = connection
        .query_row(
            "SELECT systems.name FROM systems
             JOIN applications ON applications.system_id = systems.id
             WHERE applications.id = ?1",
            [application.id.as_str()],
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

    let first =
        import_application(&mut connection, &repository, None, repository_url, None).unwrap();
    let second =
        import_application(&mut connection, &repository, None, repository_url, None).unwrap();

    let row_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM applications),
                (SELECT COUNT(*) FROM exposures)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap();
    assert_eq!(row_counts, (1, 1));
    assert_eq!(first, second);
}

#[test]
fn reimport_preserves_the_active_deployment_of_a_deployed_application() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = fixture_path("valid");
    let repository_url = "https://github.com/vitoraalmeida/vitoralmeida.tech";

    import_application(&mut connection, &repository, None, repository_url, None).unwrap();

    let application_id: String = connection
        .query_row("SELECT id FROM applications", [], |row| row.get(0))
        .unwrap();
    connection
        .execute(
            "INSERT INTO releases (id, application_id, image_reference, created_at)
             VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', ?1,
                     'ghcr.io/vitoraalmeida/vitoralmeida.tech@sha256:deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef',
                     CURRENT_TIMESTAMP)",
            [&application_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO deployments (id, application_id, release_id, type, status,
                                      requested_at, finished_at)
             VALUES ('cccccccccccccccccccccccccccccccc', ?1, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     'deploy', 'succeeded', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            [&application_id],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE applications
             SET active_deployment_id = 'cccccccccccccccccccccccccccccccc', desired_runtime_state = 'running'
             WHERE id = ?1",
            [&application_id],
        )
        .unwrap();

    let reimported =
        import_application(&mut connection, &repository, None, repository_url, None).unwrap();

    assert_eq!(reimported.id.as_str(), application_id);
    assert_eq!(
        reimported
            .active_deployment_id
            .as_ref()
            .map(|id| id.as_str()),
        Some("cccccccccccccccccccccccccccccccc")
    );
    assert_eq!(
        reimported.desired_runtime_state,
        DesiredRuntimeState::Running
    );
    let (persisted_url, application_count): (String, i64) = connection
        .query_row(
            "SELECT
                (SELECT repository_url FROM applications),
                (SELECT COUNT(*) FROM applications)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(persisted_url, repository_url);
    assert_eq!(application_count, 1);
}

#[test]
fn a_mid_aggregate_persistence_failure_rolls_back_everything() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_exposure_insert
             BEFORE INSERT ON exposures
             BEGIN
                 SELECT RAISE(ABORT, 'intentional failure');
             END",
        )
        .unwrap();

    let error = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://github.com/vitoraalmeida/vitoralmeida.tech",
        None,
    )
    .unwrap_err();

    assert!(matches!(error, ImportError::Persistence { .. }));
    let row_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM systems),
                (SELECT COUNT(*) FROM applications),
                (SELECT COUNT(*) FROM exposures)",
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
    assert_eq!(row_counts, (0, 0, 0));
}

#[test]
fn reimport_is_create_only_when_arguments_diverge() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = fixture_path("valid");
    let original_url = "https://github.com/vitoraalmeida/vitoralmeida.tech";
    let original_system = SystemName::new("system-a").unwrap();
    let divergent_system = SystemName::new("system-b").unwrap();

    let original = import_application(
        &mut connection,
        &repository,
        Some(&original_system),
        original_url,
        None,
    )
    .unwrap();

    let divergent = import_application(
        &mut connection,
        &repository,
        Some(&divergent_system),
        "https://git.example.com/different/repository",
        None,
    )
    .unwrap();

    assert_eq!(divergent.id, original.id);
    assert_eq!(divergent.system_id, original.system_id);
    assert_eq!(divergent.repository.as_str(), original_url);

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
    let (persisted_url, application_count): (String, i64) = connection
        .query_row(
            "SELECT
                (SELECT repository_url FROM applications),
                (SELECT COUNT(*) FROM applications)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(persisted_url, original_url);
    assert_eq!(application_count, 1);
}
