use pneuma::domain::application::{
    ApplicationName, ApplicationSource, RelativeManifestPath, RepositoryKind,
};
use pneuma::domain::exposure::{
    ConfirmedRoute, DomainName, ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent,
    ExposureMaterialization, ExposureMaterializationState, Visibility,
};
use pneuma::domain::identity::RuntimeInstanceId;
use pneuma::domain::release::{OciArtifact, OciRepository};
use pneuma::domain::runtime::{
    ContainerObservation, ContainerPort, ExpectedRuntimeEndpoint, HealthCheckPath,
    HealthCheckSpecification, HealthCheckStatus, ObservedRuntimeState, RuntimeSpecification,
};
use pneuma::domain::system::SystemName;

#[test]
fn validates_application_specification_value_objects() {
    assert!(ApplicationName::new("personal-site").is_ok());
    assert!(SystemName::new("Personal Site").is_err());
    assert!(ContainerPort::new(0).is_err());
    assert!(HealthCheckPath::new("health").is_err());
    assert!(HealthCheckStatus::new(600).is_err());
    assert!(DomainName::new("-invalid.example").is_err());

    let runtime = RuntimeSpecification::new(
        ContainerPort::new(8080).unwrap(),
        HealthCheckSpecification::new(
            HealthCheckPath::new("/healthz").unwrap(),
            HealthCheckStatus::new(200).unwrap(),
        ),
    );
    assert_eq!(runtime.container_port().get(), 8080);
}

#[test]
fn exposure_values_require_complete_intent_route_and_diagnostic_evidence() {
    let domain = DomainName::new("example.test").unwrap();
    assert!(ExposureIntent::new(Visibility::Public, None).is_err());
    assert!(matches!(
        ExposureIntent::new(Visibility::Internal, Some(domain.clone())),
        Ok(ExposureIntent::Internal { .. })
    ));
    assert!(ExposureConfigurationVersion::new(" \n ").is_err());
    assert!(ExposureDiagnostic::new("failed", " ").is_err());

    let route = ConfirmedRoute::new(
        RuntimeInstanceId::from("runtime-id"),
        ExposureConfigurationVersion::new("example.test {\n}\n").unwrap(),
        "2026-08-20 00:00:00".to_owned(),
    )
    .unwrap();
    let diagnostic = ExposureDiagnostic::new("failed", "route failed").unwrap();
    assert!(
        ExposureMaterialization::hydrate(ExposureMaterializationState::Active, None, None,)
            .is_err()
    );
    assert!(
        ExposureMaterialization::hydrate(
            ExposureMaterializationState::Failed,
            Some(route.clone()),
            None,
        )
        .is_err()
    );
    assert!(matches!(
        ExposureMaterialization::hydrate(
            ExposureMaterializationState::Removing,
            Some(route.clone()),
            None,
        ),
        Ok(ExposureMaterialization::Removing {
            confirmed_route: Some(_)
        })
    ));
    for materialization in [
        ExposureMaterialization::hydrate(ExposureMaterializationState::NotMaterialized, None, None),
        ExposureMaterialization::hydrate(ExposureMaterializationState::Applying, None, None),
        ExposureMaterialization::hydrate(
            ExposureMaterializationState::Applying,
            Some(route.clone()),
            None,
        ),
        ExposureMaterialization::hydrate(
            ExposureMaterializationState::Active,
            Some(route.clone()),
            None,
        ),
        ExposureMaterialization::hydrate(ExposureMaterializationState::Removing, None, None),
        ExposureMaterialization::hydrate(
            ExposureMaterializationState::Failed,
            None,
            Some(diagnostic.clone()),
        ),
        ExposureMaterialization::hydrate(
            ExposureMaterializationState::Failed,
            Some(route.clone()),
            Some(diagnostic.clone()),
        ),
        ExposureMaterialization::hydrate(
            ExposureMaterializationState::Diverged,
            None,
            Some(diagnostic.clone()),
        ),
    ] {
        assert!(materialization.is_ok());
    }
    assert!(matches!(
        ExposureMaterialization::hydrate(
            ExposureMaterializationState::Diverged,
            Some(route),
            Some(diagnostic),
        ),
        Ok(ExposureMaterialization::Diverged {
            confirmed_route: Some(_),
            ..
        })
    ));
}

#[test]
fn runtime_observations_require_a_running_loopback_endpoint() {
    let endpoint = "127.0.0.1:30000".parse().unwrap();
    assert!(ExpectedRuntimeEndpoint::new(endpoint).is_ok());
    assert!(ExpectedRuntimeEndpoint::new("0.0.0.0:30000".parse().unwrap()).is_err());
    assert!(ContainerObservation::running(endpoint).is_ok());
    assert!(ContainerObservation::running("127.0.0.1:0".parse().unwrap()).is_err());
    assert!(ContainerObservation::not_running(ObservedRuntimeState::Running).is_err());
    assert_eq!(
        ContainerObservation::missing().observed_endpoint(),
        None,
        "missing observations cannot carry endpoints"
    );
}

#[test]
fn validates_source_and_oci_repository_boundaries() {
    let manifest_path = RelativeManifestPath::new("deploy/pneuma.toml").unwrap();
    assert!(RelativeManifestPath::new("/etc/pneuma.toml").is_err());
    assert!(RelativeManifestPath::new("../pneuma.toml").is_err());
    assert!(
        ApplicationSource::new(
            RepositoryKind::Remote,
            "https://example.test/application.git",
            None,
            manifest_path,
        )
        .is_ok()
    );
    assert!(
        ApplicationSource::new(
            RepositoryKind::Remote,
            "/checkout/application",
            None,
            RelativeManifestPath::new("pneuma.toml").unwrap(),
        )
        .is_err()
    );

    assert!(OciRepository::new("registry.example:5000/team/application").is_ok());
    for invalid in [
        "registry.example/team:latest",
        "application:5000",
        "registry.example//application",
        " registry.example/app",
        "registry.example/app@sha256:aaaa",
    ] {
        assert!(OciRepository::new(invalid).is_err(), "{invalid}");
    }
    assert!(
        OciArtifact::new(
            "registry.example/team:latest",
            &format!("sha256:{}", "a".repeat(64))
        )
        .is_err()
    );
}
