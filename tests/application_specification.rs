use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::adapters::stores::PersistenceOutcome;
use pneuma::adapters::stores::application_store;
use pneuma::adapters::stores::application_store::ApplicationStoreError;
use pneuma::adapters::stores::exposure_store;
use pneuma::domain::application::DesiredRuntimeState;
use pneuma::domain::exposure::{ExposureMaterialization, Visibility};
use pneuma::domain::identity::{ApplicationId, DeploymentId};
use pneuma::use_cases::application::import_application;

#[test]
fn loads_named_source_delivery_runtime_and_health_configuration() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://github.com/vitoraalmeida/vitoralmeida.tech",
        Some("deploy/staging/pneuma.toml"),
    )
    .unwrap();

    let source = application_store::load_source(&connection, &application.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        source.repository_url(),
        "https://github.com/vitoraalmeida/vitoralmeida.tech"
    );
    assert_eq!(source.default_branch(), None);
    assert_eq!(
        source.manifest_path().as_str(),
        "deploy/staging/pneuma.toml"
    );

    let delivery = application_store::load_delivery_specification(&connection, &application.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        delivery.image_repository().as_str(),
        "ghcr.io/vitoraalmeida/vitoralmeida.tech"
    );

    let deployment = application_store::load_deployment_specification(&connection, &application.id)
        .unwrap()
        .unwrap();
    assert_eq!(deployment.application_id, application.id);
    assert_eq!(deployment.application_name.as_str(), "personal-site");
    assert_eq!(deployment.runtime.container_port().get(), 8080);
    assert_eq!(
        deployment.runtime.health_check().path().as_str(),
        "/healthz"
    );
    assert_eq!(
        deployment.runtime.health_check().expected_status().get(),
        200
    );
    assert_eq!(deployment.visibility, Visibility::Public);

    let exposure = exposure_store::load_exposure(&connection, &application.id)
        .unwrap()
        .unwrap();
    assert_eq!(exposure.application_id, application.id);
    assert_eq!(exposure.intent().visibility(), Visibility::Public);
    assert_eq!(
        exposure.intent().domain().map(|domain| domain.as_str()),
        Some("vitoralmeida.tech")
    );
    assert_eq!(
        exposure.materialization(),
        &ExposureMaterialization::NotMaterialized
    );
}

#[test]
fn rejects_invalid_persisted_specification_values_with_store_context() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();

    connection
        .execute(
            "UPDATE applications SET image_repository = 'registry.example/app:latest'",
            [],
        )
        .unwrap();
    assert!(matches!(
        application_store::load_delivery_specification(&connection, &application.id),
        Err(ApplicationStoreError::Persistence { .. })
    ));

    connection
        .execute(
            "UPDATE applications SET image_repository = 'ghcr.io/vitoraalmeida/vitoralmeida.tech'",
            [],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    connection
        .execute("UPDATE applications SET container_port = 0", [])
        .unwrap();
    assert!(matches!(
        application_store::load_deployment_specification(&connection, &application.id),
        Err(ApplicationStoreError::Persistence { .. })
    ));
}

#[test]
fn rejects_invalid_persisted_exposure_evidence_with_context() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let application_id = &application.id;
    connection
        .execute_batch("PRAGMA foreign_keys = OFF; PRAGMA ignore_check_constraints = ON;")
        .unwrap();

    connection
        .execute("UPDATE exposures SET domain = NULL", [])
        .unwrap();
    assert_invalid_exposure(&connection, application_id);

    connection
        .execute(
            "UPDATE exposures SET domain = 'vitoralmeida.tech', materialization_state = 'active'",
            [],
        )
        .unwrap();
    assert_invalid_exposure(&connection, application_id);

    connection
        .execute(
            "UPDATE exposures
             SET materialization_state = 'not_materialized', active_runtime_id = '77777777777777777777777777777777',
                 configuration_version = 'route', last_materialized_at = '2026-08-20 00:00:00'",
            [],
        )
        .unwrap();
    assert_invalid_exposure(&connection, application_id);

    connection
        .execute(
            "UPDATE exposures
             SET materialization_state = 'failed', last_error_code = 'failed',
                 last_error_message = NULL",
            [],
        )
        .unwrap();
    assert_invalid_exposure(&connection, application_id);

    connection
        .execute(
            "UPDATE exposures SET configuration_version = '   ', last_error_message = 'route failed'",
            [],
        )
        .unwrap();
    assert_invalid_exposure(&connection, application_id);

    connection
        .execute(
            "UPDATE exposures
             SET last_error_message = 'route failed', active_runtime_id = '77777777777777777777777777777777',
                 configuration_version = 'legacy route\n', last_materialized_at = '2026-08-20 00:00:00'",
            [],
        )
        .unwrap();
    let exposure = exposure_store::load_exposure(&connection, application_id)
        .unwrap()
        .unwrap();
    match exposure.materialization() {
        ExposureMaterialization::Failed {
            confirmed_route: Some(route),
            diagnostic,
        } => {
            assert_eq!(
                route.runtime_id().as_str(),
                "77777777777777777777777777777777"
            );
            assert_eq!(route.configuration_version().as_str(), "legacy route\n");
            assert_eq!(route.materialized_at(), "2026-08-20 00:00:00");
            assert_eq!(diagnostic.code(), "failed");
        }
        other => panic!("expected failed materialization retaining route, got {other:?}"),
    }

    connection
        .execute(
            "UPDATE exposures SET materialization_state = 'diverged'",
            [],
        )
        .unwrap();
    assert!(matches!(
        exposure_store::load_exposure(&connection, application_id),
        Ok(Some(exposure)) if matches!(exposure.materialization(), ExposureMaterialization::Diverged { confirmed_route: Some(_), diagnostic: _ })
    ));
}

#[test]
fn activates_a_succeeded_deployment_of_the_application_with_running_intent() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    seed_deployment(
        &connection,
        &application.id,
        SUCCEEDED_DEPLOYMENT_ID,
        "succeeded",
    );

    let transaction = connection.transaction().unwrap();
    let outcome = application_store::activate_deployment(
        &transaction,
        &application.id,
        &DeploymentId::new(SUCCEEDED_DEPLOYMENT_ID).unwrap(),
    )
    .unwrap();
    transaction.commit().unwrap();

    assert_eq!(outcome, PersistenceOutcome::Updated);
    let stored = load_application(&connection, &application.name);
    assert_eq!(
        stored
            .active_deployment_id
            .as_ref()
            .map(DeploymentId::as_str),
        Some(SUCCEEDED_DEPLOYMENT_ID)
    );
    assert_eq!(stored.desired_runtime_state, DesiredRuntimeState::Running);
}

#[test]
fn rejects_foreign_or_unsucceeded_activation_without_changing_application_state() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    seed_deployment(
        &connection,
        &application.id,
        PENDING_DEPLOYMENT_ID,
        "verifying",
    );
    connection
        .execute(
            "INSERT INTO applications (
                id, system_id, name, repository_url, manifest_path, image_repository,
                container_port, health_check_path, health_check_expected_status,
                desired_runtime_state
             )
             SELECT 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', system_id, 'other-application',
                    'https://example.test/other.git', 'pneuma.toml', 'registry.example/other',
                    8080, '/healthz', 200, 'stopped'
             FROM applications WHERE id = ?1",
            [application.id.as_str()],
        )
        .unwrap();
    seed_deployment(
        &connection,
        &ApplicationId::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
        FOREIGN_DEPLOYMENT_ID,
        "succeeded",
    );

    for (deployment, deployment_id) in [
        ("pending", PENDING_DEPLOYMENT_ID),
        ("foreign", FOREIGN_DEPLOYMENT_ID),
        ("missing", MISSING_DEPLOYMENT_ID),
    ] {
        let transaction = connection.transaction().unwrap();
        let outcome = application_store::activate_deployment(
            &transaction,
            &application.id,
            &DeploymentId::new(deployment_id).unwrap(),
        )
        .unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            outcome,
            PersistenceOutcome::Stale,
            "activation of {deployment}"
        );
    }

    let stored = load_application(&connection, &application.name);
    assert_eq!(stored.active_deployment_id, None);
    assert_eq!(stored.desired_runtime_state, DesiredRuntimeState::Stopped);
}

const SUCCEEDED_DEPLOYMENT_ID: &str = "11111111111111111111111111111111";
const PENDING_DEPLOYMENT_ID: &str = "22222222222222222222222222222222";
const FOREIGN_DEPLOYMENT_ID: &str = "33333333333333333333333333333333";
const MISSING_DEPLOYMENT_ID: &str = "44444444444444444444444444444444";

fn seed_deployment(
    connection: &rusqlite::Connection,
    application_id: &ApplicationId,
    deployment_id: &str,
    status: &str,
) {
    // Derives a distinct valid 32-hex release identity from the deployment id.
    let release_id = format!(
        "{:032x}",
        u128::from_str_radix(deployment_id, 16).unwrap() ^ (1u128 << 64)
    );
    connection
        .execute(
            "INSERT INTO releases (id, application_id, image_reference, created_at)
             VALUES (?1, ?2, 'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                     'now')",
            rusqlite::params![release_id, application_id.as_str()],
        )
        .unwrap();
    let finished_at: Option<&str> = if status == "succeeded" || status == "failed" {
        Some("now")
    } else {
        None
    };
    connection
        .execute(
            "INSERT INTO deployments (id, application_id, release_id, type, status, requested_at, finished_at)
             VALUES (?1, ?2, ?3, 'deploy', ?4, 'now', ?5)",
            rusqlite::params![
                deployment_id,
                application_id.as_str(),
                release_id,
                status,
                finished_at
            ],
        )
        .unwrap();
}

fn load_application(
    connection: &rusqlite::Connection,
    name: &pneuma::domain::application::ApplicationName,
) -> pneuma::domain::application::Application {
    application_store::load_application_by_name(connection, name)
        .unwrap()
        .unwrap()
}

fn assert_invalid_exposure(connection: &rusqlite::Connection, application_id: &ApplicationId) {
    assert!(matches!(
        exposure_store::load_exposure(connection, application_id),
        Err(exposure_store::ExposureStoreError::InvalidExposure { .. })
    ));
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
