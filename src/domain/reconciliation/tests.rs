use super::decision::PublicExposureFailureKind;
use super::*;
use crate::domain::application::{
    Application, ApplicationDeploymentSpecification, ApplicationName, DesiredRuntimeState,
};
use crate::domain::deployment::{Deployment, DeploymentLifecycle, DeploymentType};
use crate::domain::exposure::{
    ConfirmedRoute, Exposure, ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent,
    ExposureMaterialization, ExposureMaterializationState, Visibility,
};
use crate::domain::identity::{
    ApplicationId, DeploymentId, ReleaseId, RuntimeInstanceId, SystemId,
};
use crate::domain::release::{OciArtifact, Release};
use crate::domain::runtime::{
    ContainerId, ContainerObservation, ContainerPort, ExpectedRuntimeEndpoint, HealthCheckPath,
    HealthCheckSpecification, HealthCheckStatus, ObservedRuntimeState, RuntimeInstance,
    RuntimeSpecification, RuntimeState,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

const APPLICATION_ID: &str = "11111111111111111111111111111111";
const RELEASE_ID: &str = "22222222222222222222222222222222";
const DEPLOYMENT_ID: &str = "33333333333333333333333333333333";
const RUNTIME_ID: &str = "44444444444444444444444444444444";
const SYSTEM_ID: &str = "55555555555555555555555555555555";
const CONTAINER_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const CANONICAL_UNIT: &str = "canonical-unit";
const CANONICAL_ROUTE: &str = "app.example { reverse_proxy }";

fn application_name() -> ApplicationName {
    ApplicationName::new("app").unwrap()
}

fn endpoint() -> ExpectedRuntimeEndpoint {
    ExpectedRuntimeEndpoint::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 30000)).unwrap()
}

fn socket_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 30000)
}

fn deployment() -> Deployment {
    Deployment {
        id: DeploymentId::new(DEPLOYMENT_ID).unwrap(),
        application_id: ApplicationId::new(APPLICATION_ID).unwrap(),
        release_id: ReleaseId::new(RELEASE_ID).unwrap(),
        deployment_type: DeploymentType::Deploy,
        lifecycle: DeploymentLifecycle::Succeeded {
            finished_at: "2026-01-01T00:00:00Z".to_owned(),
        },
        source_revision: None,
        requested_at: "2026-01-01T00:00:00Z".to_owned(),
        started_at: Some("2026-01-01T00:00:00Z".to_owned()),
    }
}

fn release() -> Release {
    Release {
        id: ReleaseId::new(RELEASE_ID).unwrap(),
        application_id: ApplicationId::new(APPLICATION_ID).unwrap(),
        artifact: OciArtifact::parse(&format!(
            "registry.example/team/app@sha256:{}",
            "a".repeat(64)
        ))
        .unwrap(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

fn runtime() -> RuntimeInstance {
    RuntimeInstance {
        id: RuntimeInstanceId::new(RUNTIME_ID).unwrap(),
        application_id: ApplicationId::new(APPLICATION_ID).unwrap(),
        deployment_id: DeploymentId::new(DEPLOYMENT_ID).unwrap(),
        external_runtime_id: ContainerId::from(CONTAINER_ID),
        state: RuntimeState::Running,
        expected_endpoint: endpoint(),
        container_port: ContainerPort::new(8080).unwrap(),
        observed_state: ObservedRuntimeState::Running,
        observed_at: "2026-01-01T00:00:00Z".to_owned(),
        exit_code: None,
        observation_reason: None,
        retirement: None,
    }
}

fn specification() -> ApplicationDeploymentSpecification {
    ApplicationDeploymentSpecification {
        application_id: ApplicationId::new(APPLICATION_ID).unwrap(),
        application_name: application_name(),
        runtime: RuntimeSpecification::new(
            ContainerPort::new(8080).unwrap(),
            HealthCheckSpecification::new(
                HealthCheckPath::new("/healthz").unwrap(),
                HealthCheckStatus::new(200).unwrap(),
            ),
        ),
        visibility: Visibility::Internal,
    }
}

fn input(desired_state: DesiredRuntimeState, exposure: Option<Exposure>) -> ReconciliationInput {
    ReconciliationInput {
        desired: DesiredState {
            application: Application {
                id: ApplicationId::new(APPLICATION_ID).unwrap(),
                system_id: SystemId::new(SYSTEM_ID).unwrap(),
                name: application_name(),
                desired_runtime_state: desired_state,
                active_deployment_id: Some(DeploymentId::new(DEPLOYMENT_ID).unwrap()),
            },
            exposure,
        },
        persisted: PersistedState {
            blocking_deployment: None,
            active: Some(ActiveRuntime {
                deployment: deployment(),
                release: release(),
                runtime: Some(runtime()),
            }),
            specification: Some(specification()),
        },
    }
}

fn observation(
    recorded_container: ContainerObservation,
    named_container: NamedContainerObservation,
    quadlet_source: QuadletSourceObservation,
    systemd_unit: SystemdUnitObservation,
    caddy_fragment: CaddyFragmentObservation,
) -> ReconciliationObservation {
    ReconciliationObservation {
        recorded_container,
        named_container,
        quadlet_source,
        systemd_unit,
        caddy_fragment,
    }
}

fn everything_missing() -> ReconciliationObservation {
    observation(
        ContainerObservation::missing(),
        NamedContainerObservation::Missing,
        QuadletSourceObservation::Missing,
        SystemdUnitObservation::Missing,
        CaddyFragmentObservation::Missing,
    )
}

fn expectations(canonical_route_fragment: Option<String>) -> ReconciliationExpectations {
    ReconciliationExpectations {
        container_name: format!("pneuma-app-{DEPLOYMENT_ID}"),
        canonical_quadlet_contents: CANONICAL_UNIT.to_owned(),
        canonical_route_fragment,
    }
}

fn recreated_container(
    name: String,
    image_reference: String,
    application_label: Option<String>,
    image_digest_label: Option<String>,
    container_observation: ContainerObservation,
) -> NamedContainerObservation {
    NamedContainerObservation::Present {
        id: ContainerId::from(CONTAINER_ID),
        name,
        image_reference,
        application_label,
        image_digest_label,
        observation: container_observation,
    }
}

fn named_present(container_observation: ContainerObservation) -> NamedContainerObservation {
    let artifact = release().artifact;
    recreated_container(
        format!("/pneuma-app-{DEPLOYMENT_ID}"),
        artifact.reference().to_owned(),
        Some("app".to_owned()),
        Some(artifact.digest().to_owned()),
        container_observation,
    )
}

fn quadlet(contents: &str) -> QuadletSourceObservation {
    QuadletSourceObservation::Present {
        contents: contents.to_owned(),
    }
}

fn internal_exposure(state: ExposureMaterializationState) -> Exposure {
    Exposure::new(
        ApplicationId::new(APPLICATION_ID).unwrap(),
        ExposureIntent::new(Visibility::Internal, None).unwrap(),
        ExposureMaterialization::hydrate(state, Option::<ConfirmedRoute>::None, None).unwrap(),
    )
}

fn public_exposure(state: ExposureMaterializationState, confirmed_route: bool) -> Exposure {
    let route = confirmed_route.then(|| {
        ConfirmedRoute::new(
            crate::domain::identity::RuntimeInstanceId::new(RUNTIME_ID).unwrap(),
            ExposureConfigurationVersion::new(CANONICAL_ROUTE).unwrap(),
            "2026-01-01T00:00:00Z".to_owned(),
        )
        .unwrap()
    });
    let diagnostic = matches!(
        state,
        ExposureMaterializationState::Failed | ExposureMaterializationState::Diverged
    )
    .then(|| ExposureDiagnostic::new("test", "test diagnostic").unwrap());
    Exposure::new(
        ApplicationId::new(APPLICATION_ID).unwrap(),
        ExposureIntent::new(
            Visibility::Public,
            Some(crate::domain::exposure::DomainName::new("app.example").unwrap()),
        )
        .unwrap(),
        ExposureMaterialization::hydrate(state, route, diagnostic).unwrap(),
    )
}

// Characterizes top-level `decide` rules: converged stopped state, the running-desired
// manual-intervention fallback, and refusal of drift no safe rule covers.
mod convergence {
    use super::*;

    #[test]
    fn stopped_application_fully_absent_is_in_sync() {
        let input = input(DesiredRuntimeState::Stopped, None);
        let decision = decide(&input, &everything_missing(), &expectations(None)).unwrap();
        assert_eq!(decision, ReconciliationDecision::InSync);
    }

    #[test]
    fn stopped_drift_without_a_safe_rule_is_refused() {
        let input = input(DesiredRuntimeState::Stopped, None);
        let observed = ReconciliationObservation {
            caddy_fragment: CaddyFragmentObservation::Present {
                contents: "stale".to_owned(),
            },
            ..everything_missing()
        };
        let error = decide(&input, &observed, &expectations(None)).unwrap_err();
        assert!(matches!(error, ReconciliationDecisionError::UnhandledDrift));
    }

    #[test]
    fn known_stopped_recorded_runtime_is_not_silently_restarted_while_running_is_desired() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::not_running(ObservedRuntimeState::Stopped).unwrap(),
            NamedContainerObservation::Missing,
            QuadletSourceObservation::Missing,
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(_)
        ));
    }

    #[test]
    fn unknown_recorded_runtime_state_requires_manual_intervention_while_running_is_desired() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::not_running(ObservedRuntimeState::Unknown {
                status: "restarting".to_owned(),
            })
            .unwrap(),
            NamedContainerObservation::Missing,
            QuadletSourceObservation::Missing,
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(_)
        ));
    }

    #[test]
    fn unknown_recorded_runtime_state_is_refused_when_stopped_is_desired() {
        let input = input(DesiredRuntimeState::Stopped, None);
        let observed = observation(
            ContainerObservation::not_running(ObservedRuntimeState::Unknown {
                status: "restarting".to_owned(),
            })
            .unwrap(),
            NamedContainerObservation::Missing,
            QuadletSourceObservation::Missing,
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let error = decide(&input, &observed, &expectations(None)).unwrap_err();
        assert!(matches!(error, ReconciliationDecisionError::UnhandledDrift));
    }

    // Pins the preserved precedence of the externally relied-on flow: only a
    // confirmed public route reaches `InSync`, so a fully healthy running
    // internal application still falls through to manual intervention.
    #[test]
    fn converged_running_internal_runtime_reports_manual_intervention_instead_of_a_silent_no_op() {
        let input = input(
            DesiredRuntimeState::Running,
            Some(internal_exposure(
                ExposureMaterializationState::NotMaterialized,
            )),
        );
        let observed = observation(
            ContainerObservation::running(socket_addr()).unwrap(),
            named_present(ContainerObservation::running(socket_addr()).unwrap()),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Present {
                active_state: "active".to_owned(),
            },
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RequireManualIntervention(
                "runtime identity or configuration differs from persisted intent".to_owned()
            )
        );
    }
}
// Characterizes `classify_runtime_identity_repair` and the `matches_expected_runtime`
// predicate it shares with post-effect confirmation.
mod runtime_identity_repair {
    use super::*;

    #[test]
    fn proven_recreated_container_repairs_runtime_identity() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            named_present(ContainerObservation::running(socket_addr()).unwrap()),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RepairRuntime(RuntimeIdentityRepair {
                runtime_id: RuntimeInstanceId::new(RUNTIME_ID).unwrap(),
                container_id: ContainerId::from(CONTAINER_ID),
            })
        );
    }

    // Pins the canonical identity predicate as a table: each wrong identity
    // dimension independently fails the match, absence never matches, and
    // Podman's leading-slash name form is normalized inside the predicate.
    #[test]
    fn expected_runtime_matching_fails_for_each_wrong_identity_dimension() {
        let artifact = release().artifact;
        let matches = |observed: &NamedContainerObservation| {
            observed.matches_expected_runtime(
                &format!("pneuma-app-{DEPLOYMENT_ID}"),
                &artifact,
                "app",
                socket_addr(),
            )
        };
        let matching = |name: String| {
            recreated_container(
                name,
                artifact.reference().to_owned(),
                Some("app".to_owned()),
                Some(artifact.digest().to_owned()),
                ContainerObservation::running(socket_addr()).unwrap(),
            )
        };

        assert!(matches(&matching(format!("/pneuma-app-{DEPLOYMENT_ID}"))));
        assert!(matches(&matching(format!("pneuma-app-{DEPLOYMENT_ID}"))));

        let foreign_endpoint = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 30001);
        let wrong_dimensions: [(&str, NamedContainerObservation); 7] = [
            (
                "state",
                recreated_container(
                    format!("/pneuma-app-{DEPLOYMENT_ID}"),
                    artifact.reference().to_owned(),
                    Some("app".to_owned()),
                    Some(artifact.digest().to_owned()),
                    ContainerObservation::not_running(ObservedRuntimeState::Stopped).unwrap(),
                ),
            ),
            ("name", matching("/pneuma-app-foreign".to_owned())),
            (
                "image",
                recreated_container(
                    format!("/pneuma-app-{DEPLOYMENT_ID}"),
                    format!("registry.example/team/app@sha256:{}", "b".repeat(64)),
                    Some("app".to_owned()),
                    Some(artifact.digest().to_owned()),
                    ContainerObservation::running(socket_addr()).unwrap(),
                ),
            ),
            (
                "application label",
                recreated_container(
                    format!("/pneuma-app-{DEPLOYMENT_ID}"),
                    artifact.reference().to_owned(),
                    Some("other".to_owned()),
                    Some(artifact.digest().to_owned()),
                    ContainerObservation::running(socket_addr()).unwrap(),
                ),
            ),
            (
                "digest label",
                recreated_container(
                    format!("/pneuma-app-{DEPLOYMENT_ID}"),
                    artifact.reference().to_owned(),
                    Some("app".to_owned()),
                    Some(format!("sha256:{}", "c".repeat(64))),
                    ContainerObservation::running(socket_addr()).unwrap(),
                ),
            ),
            (
                "endpoint",
                recreated_container(
                    format!("/pneuma-app-{DEPLOYMENT_ID}"),
                    artifact.reference().to_owned(),
                    Some("app".to_owned()),
                    Some(artifact.digest().to_owned()),
                    ContainerObservation::running(foreign_endpoint).unwrap(),
                ),
            ),
            ("missing", NamedContainerObservation::Missing),
        ];
        for (dimension, observed) in wrong_dimensions {
            assert!(
                !matches(&observed),
                "a wrong {dimension} must not count as the expected runtime"
            );
        }
    }

    #[test]
    fn recreated_container_with_divergent_quadlet_is_not_auto_repaired() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            named_present(ContainerObservation::running(socket_addr()).unwrap()),
            quadlet("divergent-unit"),
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(reason)
                if reason == "runtime identity or configuration differs from persisted intent"
        ));
    }

    #[test]
    fn recreated_container_under_a_foreign_name_is_not_silently_adopted() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            recreated_container(
                "/pneuma-app-foreign".to_owned(),
                release().artifact.reference().to_owned(),
                Some("app".to_owned()),
                Some(release().artifact.digest().to_owned()),
                ContainerObservation::running(socket_addr()).unwrap(),
            ),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(_)
        ));
    }

    #[test]
    fn recreated_container_from_a_foreign_image_is_not_silently_adopted() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            recreated_container(
                format!("/pneuma-app-{DEPLOYMENT_ID}"),
                format!("registry.example/team/app@sha256:{}", "b".repeat(64)),
                Some("app".to_owned()),
                Some(release().artifact.digest().to_owned()),
                ContainerObservation::running(socket_addr()).unwrap(),
            ),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(_)
        ));
    }

    #[test]
    fn recreated_container_without_the_application_label_is_not_silently_adopted() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            recreated_container(
                format!("/pneuma-app-{DEPLOYMENT_ID}"),
                release().artifact.reference().to_owned(),
                None,
                Some(release().artifact.digest().to_owned()),
                ContainerObservation::running(socket_addr()).unwrap(),
            ),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(_)
        ));
    }

    #[test]
    fn recreated_container_without_the_artifact_digest_label_is_not_silently_adopted() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            recreated_container(
                format!("/pneuma-app-{DEPLOYMENT_ID}"),
                release().artifact.reference().to_owned(),
                Some("app".to_owned()),
                None,
                ContainerObservation::running(socket_addr()).unwrap(),
            ),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(_)
        ));
    }

    #[test]
    fn recreated_container_on_a_foreign_endpoint_is_not_silently_adopted() {
        let input = input(DesiredRuntimeState::Running, None);
        let foreign_endpoint = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 30001);
        let observed = observation(
            ContainerObservation::missing(),
            named_present(ContainerObservation::running(foreign_endpoint).unwrap()),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Missing,
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(_)
        ));
    }
}
// Characterizes `classify_runtime_rematerialization` and its conservative reading of
// systemd's generated-unit states.
mod runtime_rematerialization {
    use super::*;

    #[test]
    fn absent_materialization_remateralizes_and_rewrites_the_unit() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = everything_missing();
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RematerializeRuntime(RuntimeRematerialization {
                unit_needs_write: true,
            })
        );
    }

    #[test]
    fn canonical_quadlet_source_skips_the_unit_rewrite() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            NamedContainerObservation::Missing,
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Present {
                active_state: "inactive".to_owned(),
            },
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RematerializeRuntime(RuntimeRematerialization {
                unit_needs_write: false,
            })
        );
    }

    #[test]
    fn active_generated_unit_blocks_rematerialization() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            NamedContainerObservation::Missing,
            QuadletSourceObservation::Missing,
            SystemdUnitObservation::Present {
                active_state: "active".to_owned(),
            },
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(_)
        ));
    }

    // A systemd state outside the known not-running vocabulary (transient or
    // introduced by a future systemd version) must never be silently adopted as
    // startable; reconciliation refuses to guess.
    #[test]
    fn unknown_or_transient_generated_unit_states_block_rematerialization() {
        for active_state in ["activating", "reloading", "maintenance"] {
            let input = input(DesiredRuntimeState::Running, None);
            let observed = observation(
                ContainerObservation::missing(),
                NamedContainerObservation::Missing,
                QuadletSourceObservation::Missing,
                SystemdUnitObservation::Present {
                    active_state: active_state.to_owned(),
                },
                CaddyFragmentObservation::Missing,
            );
            let decision = decide(&input, &observed, &expectations(None)).unwrap();
            assert!(
                matches!(
                    decision,
                    ReconciliationDecision::RequireManualIntervention(_)
                ),
                "expected manual intervention for active state `{active_state}`"
            );
        }
    }

    #[test]
    fn failed_generated_unit_permits_rematerialization() {
        let input = input(DesiredRuntimeState::Running, None);
        let observed = observation(
            ContainerObservation::missing(),
            NamedContainerObservation::Missing,
            QuadletSourceObservation::Missing,
            SystemdUnitObservation::Present {
                active_state: "failed".to_owned(),
            },
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RematerializeRuntime(RuntimeRematerialization {
                // The Quadlet source is missing in this scenario, so execution must rewrite it.
                unit_needs_write: true,
            })
        );
    }
}
// Characterizes `classify_exposure` and `classify_public_exposure`: internal route
// removal, public confirmation/materialization, and recorded failures.
mod exposure {
    use super::*;

    #[test]
    fn diverged_exposure_requires_manual_intervention() {
        let input = input(
            DesiredRuntimeState::Stopped,
            Some(public_exposure(
                ExposureMaterializationState::Diverged,
                false,
            )),
        );
        let observed = ReconciliationObservation {
            caddy_fragment: CaddyFragmentObservation::Present {
                contents: "stale".to_owned(),
            },
            ..everything_missing()
        };
        let decision = decide(
            &input,
            &observed,
            &expectations(Some(CANONICAL_ROUTE.to_owned())),
        )
        .unwrap();
        assert!(matches!(
            decision,
            ReconciliationDecision::RequireManualIntervention(reason)
                if reason == "exposure materialization diverged and requires manual intervention"
        ));
    }

    #[test]
    fn internal_exposure_with_stale_fragment_removes_it() {
        let input = input(
            DesiredRuntimeState::Stopped,
            Some(internal_exposure(ExposureMaterializationState::Applying)),
        );
        let observed = everything_missing();
        // A present fragment is the only drift here, so the stopped-in-sync rule does not apply.
        let observed = ReconciliationObservation {
            caddy_fragment: CaddyFragmentObservation::Present {
                contents: "stale".to_owned(),
            },
            ..observed
        };
        let decision = decide(&input, &observed, &expectations(None)).unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RemoveInternalRoute {
                expected_state: ExposureMaterializationState::Applying,
            }
        );
    }

    #[test]
    fn public_exposure_without_runtime_records_failure() {
        let mut input = input(
            DesiredRuntimeState::Running,
            Some(public_exposure(
                ExposureMaterializationState::NotMaterialized,
                false,
            )),
        );
        input.persisted.active = None;
        let observed = everything_missing();
        let decision = decide(
            &input,
            &observed,
            &expectations(Some(CANONICAL_ROUTE.to_owned())),
        )
        .unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RecordPublicExposureFailure(PublicExposureFailure {
                expected_state: ExposureMaterializationState::NotMaterialized,
                kind: PublicExposureFailureKind::RuntimeMissing,
            })
        );
    }

    #[test]
    fn public_exposure_with_unhealthy_runtime_records_failure() {
        let input = input(
            DesiredRuntimeState::Running,
            Some(public_exposure(
                ExposureMaterializationState::Applying,
                false,
            )),
        );
        // An active generated unit blocks rematerialization so the public rule is reached.
        let observed = observation(
            ContainerObservation::missing(),
            NamedContainerObservation::Missing,
            QuadletSourceObservation::Missing,
            SystemdUnitObservation::Present {
                active_state: "active".to_owned(),
            },
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(
            &input,
            &observed,
            &expectations(Some(CANONICAL_ROUTE.to_owned())),
        )
        .unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RecordPublicExposureFailure(PublicExposureFailure {
                expected_state: ExposureMaterializationState::Applying,
                kind: PublicExposureFailureKind::RuntimeNotHealthy,
            })
        );
    }

    #[test]
    fn public_exposure_with_an_active_bundle_missing_its_runtime_records_failure() {
        let mut input = input(
            DesiredRuntimeState::Running,
            Some(public_exposure(
                ExposureMaterializationState::NotMaterialized,
                false,
            )),
        );
        input.persisted.active.as_mut().unwrap().runtime = None;
        // An active generated unit blocks rematerialization so the public rule is reached.
        let observed = observation(
            ContainerObservation::missing(),
            NamedContainerObservation::Missing,
            QuadletSourceObservation::Missing,
            SystemdUnitObservation::Present {
                active_state: "active".to_owned(),
            },
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(
            &input,
            &observed,
            &expectations(Some(CANONICAL_ROUTE.to_owned())),
        )
        .unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::RecordPublicExposureFailure(PublicExposureFailure {
                expected_state: ExposureMaterializationState::NotMaterialized,
                kind: PublicExposureFailureKind::RuntimeMissing,
            })
        );
    }

    #[test]
    fn confirmed_public_route_is_in_sync() {
        let input = input(
            DesiredRuntimeState::Running,
            Some(public_exposure(ExposureMaterializationState::Active, true)),
        );
        let observed = observation(
            ContainerObservation::running(socket_addr()).unwrap(),
            named_present(ContainerObservation::running(socket_addr()).unwrap()),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Present {
                active_state: "active".to_owned(),
            },
            CaddyFragmentObservation::Present {
                contents: CANONICAL_ROUTE.to_owned(),
            },
        );
        let decision = decide(
            &input,
            &observed,
            &expectations(Some(CANONICAL_ROUTE.to_owned())),
        )
        .unwrap();
        assert_eq!(decision, ReconciliationDecision::InSync);
    }

    #[test]
    fn drifted_public_route_materializes_the_canonical_fragment() {
        let input = input(
            DesiredRuntimeState::Running,
            Some(public_exposure(ExposureMaterializationState::Active, true)),
        );
        let observed = observation(
            ContainerObservation::running(socket_addr()).unwrap(),
            named_present(ContainerObservation::running(socket_addr()).unwrap()),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Present {
                active_state: "active".to_owned(),
            },
            CaddyFragmentObservation::Missing,
        );
        let decision = decide(
            &input,
            &observed,
            &expectations(Some(CANONICAL_ROUTE.to_owned())),
        )
        .unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::MaterializePublicRoute {
                expected_state: ExposureMaterializationState::Active,
            }
        );
    }

    #[test]
    fn canonical_public_fragment_with_an_unconfirmed_route_materializes_it() {
        // An applying materialization has no confirmed route yet; matching bytes
        // still require an explicit confirmation pass.
        let input = input(
            DesiredRuntimeState::Running,
            Some(public_exposure(
                ExposureMaterializationState::Applying,
                false,
            )),
        );
        let observed = observation(
            ContainerObservation::running(socket_addr()).unwrap(),
            named_present(ContainerObservation::running(socket_addr()).unwrap()),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Present {
                active_state: "active".to_owned(),
            },
            CaddyFragmentObservation::Present {
                contents: CANONICAL_ROUTE.to_owned(),
            },
        );
        let decision = decide(
            &input,
            &observed,
            &expectations(Some(CANONICAL_ROUTE.to_owned())),
        )
        .unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::MaterializePublicRoute {
                expected_state: ExposureMaterializationState::Applying,
            }
        );
    }

    #[test]
    fn divergent_public_fragment_contents_are_rematerialed_from_the_canonical_bytes() {
        let input = input(
            DesiredRuntimeState::Running,
            Some(public_exposure(ExposureMaterializationState::Active, true)),
        );
        let observed = observation(
            ContainerObservation::running(socket_addr()).unwrap(),
            named_present(ContainerObservation::running(socket_addr()).unwrap()),
            quadlet(CANONICAL_UNIT),
            SystemdUnitObservation::Present {
                active_state: "active".to_owned(),
            },
            CaddyFragmentObservation::Present {
                contents: "divergent-route".to_owned(),
            },
        );
        let decision = decide(
            &input,
            &observed,
            &expectations(Some(CANONICAL_ROUTE.to_owned())),
        )
        .unwrap();
        assert_eq!(
            decision,
            ReconciliationDecision::MaterializePublicRoute {
                expected_state: ExposureMaterializationState::Active,
            }
        );
    }
}
