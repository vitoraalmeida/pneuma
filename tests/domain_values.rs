use pneuma::domain::application::ApplicationName;
use pneuma::domain::exposure::{
    ConfirmedRoute, DomainName, ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent,
    ExposureMaterialization, ExposureMaterializationState, Visibility,
};
use pneuma::domain::git::CommitSha;
use pneuma::domain::git::{ApplicationSource, RelativeManifestPath, RepositoryKind};
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
fn enforces_catalog_name_length_limits() {
    let longest_allowed = format!("a{}b", "c".repeat(61));
    assert_eq!(longest_allowed.len(), 63);
    assert!(ApplicationName::new(&longest_allowed).is_ok());
    assert!(SystemName::new(&longest_allowed).is_ok());

    let too_long = format!("{longest_allowed}c");
    assert_eq!(too_long.len(), 64);
    assert!(ApplicationName::new(&too_long).is_err());
    assert!(SystemName::new(&too_long).is_err());
}

#[test]
fn accepts_only_the_http_status_range_for_health_checks() {
    assert!(HealthCheckStatus::new(99).is_err());
    assert!(HealthCheckStatus::new(100).is_ok());
    assert!(HealthCheckStatus::new(599).is_ok());
    assert!(HealthCheckStatus::new(0).is_err());
}

#[test]
fn validates_immutable_git_commit_identity() {
    assert!(CommitSha::new(&"a".repeat(40)).is_ok());
    assert!(CommitSha::new(&"A".repeat(40)).is_err());
    assert!(CommitSha::new("short").is_err());
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

#[test]
fn parses_only_digest_pinned_oci_artifact_references() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let artifact = OciArtifact::parse(&format!("registry.example/app@{digest}")).unwrap();
    assert_eq!(artifact.repository(), "registry.example/app");
    assert_eq!(artifact.digest(), digest);

    for invalid in [
        "registry.example/app",
        "registry.example/app@sha256:aaaa",
        &format!("registry.example/app@sha256:{}", "A".repeat(64)),
        &format!("registry.example/app@sha512:{}", "a".repeat(64)),
        &format!("@sha256:{}", "a".repeat(64)),
    ] {
        assert!(OciArtifact::parse(invalid).is_err(), "{invalid}");
    }
}
