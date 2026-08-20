use pneuma::domain::application::{
    ApplicationName, ApplicationSource, ContainerPort, HealthCheckPath, HealthCheckSpecification,
    HealthCheckStatus, RelativeManifestPath, RepositoryKind, RuntimeSpecification, SystemName,
};
use pneuma::domain::exposure::DomainName;
use pneuma::domain::release::{OciArtifact, OciRepository};

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
