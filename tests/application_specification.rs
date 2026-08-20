use std::path::{Path, PathBuf};

use pneuma::adapters::database;
use pneuma::adapters::stores::application_store;
use pneuma::domain::application::RepositoryKind;
use pneuma::domain::delivery::DeliveryType;
use pneuma::domain::exposure::{ExposureMaterializationState, Visibility};
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
        source.repository_url,
        "https://github.com/vitoraalmeida/vitoralmeida.tech"
    );
    assert_eq!(source.repository_kind, RepositoryKind::Remote);
    assert_eq!(source.default_branch, None);
    assert_eq!(source.manifest_path, "deploy/staging/pneuma.toml");

    let delivery =
        application_store::load_delivery_specification(&connection, application.id.as_str())
            .unwrap()
            .unwrap();
    assert_eq!(delivery.delivery_type, DeliveryType::Oci);
    assert_eq!(
        delivery.image_repository,
        "ghcr.io/vitoraalmeida/vitoralmeida.tech"
    );

    let deployment =
        application_store::load_deployment_specification(&connection, application.id.as_str())
            .unwrap()
            .unwrap();
    assert_eq!(deployment.application_id, application.id);
    assert_eq!(deployment.application_name, "personal-site");
    assert_eq!(deployment.runtime.container_port, 8080);
    assert_eq!(deployment.runtime.health_check.path, "/healthz");
    assert_eq!(deployment.runtime.health_check.expected_status, 200);
    assert_eq!(deployment.visibility, Visibility::Public);

    let exposure = application_store::load_exposure(&connection, application.id.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(exposure.application_id, application.id);
    assert_eq!(exposure.desired_visibility, Visibility::Public);
    assert_eq!(exposure.domain.as_deref(), Some("vitoralmeida.tech"));
    assert_eq!(exposure.active_runtime_id, None);
    assert_eq!(
        exposure.materialization_state,
        ExposureMaterializationState::NotMaterialized
    );
    assert_eq!(exposure.configuration_version, None);
    assert_eq!(exposure.last_materialized_at, None);
    assert_eq!(exposure.last_error_code, None);
    assert_eq!(exposure.last_error_message, None);
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
