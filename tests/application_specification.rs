use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::adapters::stores::PersistenceOutcome;
use pneuma::adapters::stores::application_store;
use pneuma::adapters::stores::application_store::ApplicationStoreError;
use pneuma::adapters::stores::exposure_store;
use pneuma::domain::application::DesiredRuntimeState;
use pneuma::domain::exposure::{ExposureMaterialization, Visibility};
use pneuma::domain::git::RepositoryKind;
use pneuma::domain::identity::{ApplicationId, DeploymentId};
use pneuma::domain::release::DeliveryType;
use pneuma::use_cases::application::import_application;

#[test]
fn loads_named_source_delivery_runtime_and_health_configuration() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("valid"),
        None,
        Some("https://github.com/vitoraalmeida/vitoralmeida.tech"),
        Some("deploy/staging/pneuma.toml"),
    )
    .unwrap();

    let source = application_store::load_source(&connection, &application.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        source.repository_location(),
        "https://github.com/vitoraalmeida/vitoralmeida.tech"
    );
    assert_eq!(source.repository_kind(), RepositoryKind::Remote);
    assert_eq!(source.default_branch(), None);
    assert_eq!(
        source.manifest_path().as_str(),
        "deploy/staging/pneuma.toml"
    );

    let delivery = application_store::load_delivery_specification(&connection, &application.id)
        .unwrap()
        .unwrap();
    assert_eq!(delivery.delivery_type(), DeliveryType::Oci);
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
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();

    connection.execute("UPDATE application_delivery_specs SET image_repository = 'registry.example/app:latest'", []).unwrap();
    assert!(matches!(
        application_store::load_delivery_specification(&connection, &application.id),
        Err(ApplicationStoreError::Persistence { .. })
    ));

    connection.execute("UPDATE application_delivery_specs SET image_repository = 'ghcr.io/vitoraalmeida/vitoralmeida.tech'", []).unwrap();
    connection
        .execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    connection
        .execute(
            "UPDATE application_runtime_specs SET container_port = 0",
            [],
        )
        .unwrap();
    assert!(matches!(
        application_store::load_deployment_specification(&connection, &application.id),
        Err(ApplicationStoreError::Persistence { .. })
    ));
}

#[test]
fn rejects_invalid_persisted_exposure_evidence_with_context() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
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
             SET materialization_state = 'not_materialized', active_runtime_id = 'prior-runtime',
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
             SET last_error_message = 'route failed', active_runtime_id = 'prior-runtime',
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
            assert_eq!(route.runtime_id().as_str(), "prior-runtime");
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
fn loads_historical_internal_removal_timestamp_without_a_confirmed_route() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    connection
        .execute(
            "UPDATE exposures
             SET desired_visibility = 'internal',
                 materialization_state = 'not_materialized',
                 active_runtime_id = NULL,
                 configuration_version = NULL,
                 last_materialized_at = '2026-08-20 00:00:00'",
            [],
        )
        .unwrap();

    let exposure = exposure_store::load_exposure(&connection, &application.id)
        .unwrap()
        .unwrap();
    assert!(matches!(
        exposure.materialization(),
        ExposureMaterialization::NotMaterialized
    ));
}

#[test]
fn activates_a_succeeded_deployment_of_the_application_with_running_intent() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    seed_deployment(
        &connection,
        &application.id,
        "succeeded-deployment",
        "succeeded",
    );

    let transaction = connection.transaction().unwrap();
    let outcome = application_store::activate_deployment(
        &transaction,
        &application.id,
        &DeploymentId::from("succeeded-deployment"),
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
        Some("succeeded-deployment")
    );
    assert_eq!(stored.desired_runtime_state, DesiredRuntimeState::Running);
}

#[test]
fn rejects_foreign_or_unsucceeded_activation_without_changing_application_state() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    seed_deployment(
        &connection,
        &application.id,
        "pending-deployment",
        "verifying",
    );
    connection
        .execute(
            "INSERT INTO applications (
                id, name, desired_runtime_state, spec_version, created_at, updated_at
             ) VALUES ('other-application', 'other-application', 'stopped', 1, 'now', 'now')",
            [],
        )
        .unwrap();
    seed_deployment(
        &connection,
        &ApplicationId::from("other-application"),
        "foreign-deployment",
        "succeeded",
    );

    for deployment in [
        "pending-deployment",
        "foreign-deployment",
        "missing-deployment",
    ] {
        let transaction = connection.transaction().unwrap();
        let outcome = application_store::activate_deployment(
            &transaction,
            &application.id,
            &DeploymentId::from(deployment),
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

fn seed_deployment(
    connection: &rusqlite::Connection,
    application_id: &ApplicationId,
    deployment_id: &str,
    status: &str,
) {
    connection
        .execute(
            "INSERT INTO releases (
                id, application_id, image_repository, image_digest, image_reference, created_at
             ) VALUES (?1, ?2, 'registry.example/app',
                       'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                       'registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                       'now')",
            rusqlite::params![format!("release-{deployment_id}"), application_id.as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO deployments (id, application_id, release_id, type, status)
             VALUES (?1, ?2, ?3, 'deploy', ?4)",
            rusqlite::params![
                deployment_id,
                application_id.as_str(),
                format!("release-{deployment_id}"),
                status
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
