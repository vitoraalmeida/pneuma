use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::adapters::stores::application_store;
use pneuma::adapters::stores::application_store::ApplicationStoreError;
use pneuma::domain::application::RepositoryKind;
use pneuma::domain::delivery::DeliveryType;
use pneuma::domain::exposure::{ExposureMaterialization, Visibility};
use pneuma::use_cases::application_import::import_application;

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

    let source = application_store::load_source(&connection, application.id.as_str())
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

    let delivery =
        application_store::load_delivery_specification(&connection, application.id.as_str())
            .unwrap()
            .unwrap();
    assert_eq!(delivery.delivery_type(), DeliveryType::Oci);
    assert_eq!(
        delivery.image_repository().as_str(),
        "ghcr.io/vitoraalmeida/vitoralmeida.tech"
    );

    let deployment =
        application_store::load_deployment_specification(&connection, application.id.as_str())
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

    let exposure = application_store::load_exposure(&connection, application.id.as_str())
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
        application_store::load_delivery_specification(&connection, application.id.as_str()),
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
        application_store::load_deployment_specification(&connection, application.id.as_str()),
        Err(ApplicationStoreError::Persistence { .. })
    ));
}

#[test]
fn rejects_invalid_persisted_exposure_evidence_with_context() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("valid"), None, None, None).unwrap();
    let application_id = application.id.as_str();
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
    let exposure = application_store::load_exposure(&connection, application_id)
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
        application_store::load_exposure(&connection, application_id),
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

    let exposure = application_store::load_exposure(&connection, application.id.as_str())
        .unwrap()
        .unwrap();
    assert!(matches!(
        exposure.materialization(),
        ExposureMaterialization::NotMaterialized
    ));
}

fn assert_invalid_exposure(connection: &rusqlite::Connection, application_id: &str) {
    assert!(matches!(
        application_store::load_exposure(connection, application_id),
        Err(application_store::ExposureStoreError::InvalidExposure { .. })
    ));
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
