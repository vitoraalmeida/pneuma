use crate::domain::application::{
    Application, ApplicationDeploymentSpecification, DesiredRuntimeState,
};
use crate::domain::deployment::Deployment;
use crate::domain::exposure::{
    Exposure, ExposureConfigurationVersion, ExposureMaterializationState,
    InvalidExposureConfigurationVersion, Visibility,
};
use crate::domain::identity::RuntimeInstanceId;
use crate::domain::release::Release;
use crate::domain::runtime::{
    ContainerId, ContainerObservation, ObservedRuntimeState, RuntimeInstance,
};

// Desired intent as recorded in SQLite: which runtime state and route Pneuma should converge to.
#[derive(Debug)]
pub struct DesiredState {
    pub application: Application,
    pub exposure: Option<Exposure>,
}

// Persisted bookkeeping recorded in SQLite: coordination and confirmation facts
// that describe workflow state rather than requested intent.
#[derive(Debug)]
pub struct PersistedState {
    pub blocking_deployment: Option<Deployment>,
    pub active: Option<ActiveRuntime>,
    pub specification: Option<ApplicationDeploymentSpecification>,
}

// Groups SQLite-produced facts by origin so intent is distinguishable from
// persisted bookkeeping; observed Podman/systemd/Caddy facts stay separate in
// `ReconciliationObservation`.
#[derive(Debug)]
pub struct ReconciliationInput {
    pub desired: DesiredState,
    pub persisted: PersistedState,
}

// Couples the active logical deployment with its immutable artifact and retained runtime identity.
#[derive(Debug)]
pub struct ActiveRuntime {
    pub deployment: Deployment,
    pub release: Release,
    pub runtime: Option<RuntimeInstance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Distinguishes a missing stable container name from a present materialization with inspectable identity.
pub enum NamedContainerObservation {
    Missing,
    Present {
        id: ContainerId,
        name: String,
        image_reference: String,
        application_label: Option<String>,
        image_digest_label: Option<String>,
        observation: ContainerObservation,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Preserves the source bytes needed to classify a Quadlet as canonical or divergent later.
pub enum QuadletSourceObservation {
    Missing,
    Present { contents: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Retains systemd's generated-unit facts without treating an absent unit as an operational failure.
pub enum SystemdUnitObservation {
    Missing,
    Present { active_state: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Preserves Caddy fragment absence separately from its exact on-disk representation.
pub enum CaddyFragmentObservation {
    Missing,
    Present { contents: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Captures the read-only external facts observed from each authority:
// Podman (recorded and named containers), systemd/Quadlet (unit source and
// generated unit), and Caddy (materialized fragment).
pub struct ReconciliationObservation {
    pub recorded_container: ContainerObservation,
    pub named_container: NamedContainerObservation,
    pub quadlet_source: QuadletSourceObservation,
    pub systemd_unit: SystemdUnitObservation,
    pub caddy_fragment: CaddyFragmentObservation,
}

// Boundary-rendered external representations that observed files must match to
// count as canonical. The adapters own the exact bytes; the pure decision only
// compares them against observations.
#[derive(Debug)]
pub struct ReconciliationExpectations {
    pub container_name: String,
    pub canonical_quadlet_contents: String,
    // Some only when a public exposure names a domain and an active runtime endpoint exists.
    pub canonical_route_fragment: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
// What reconciliation should do next for one application, decided purely from
// persisted facts, observations, and boundary expectations before any effect.
pub enum ReconciliationDecision {
    InSync,
    RepairRuntime(RuntimeIdentityRepair),
    RematerializeRuntime(RuntimeRematerialization),
    RemoveInternalRoute {
        expected_state: ExposureMaterializationState,
    },
    MaterializePublicRoute {
        expected_state: ExposureMaterializationState,
    },
    RecordPublicExposureFailure(PublicExposureFailure),
    RequireManualIntervention(String),
}

#[derive(Debug, PartialEq, Eq)]
// A fully proven recreated container carrying the persisted logical identity;
// execution confirms it with a CAS swap of the recorded container id.
pub struct RuntimeIdentityRepair {
    pub runtime_id: RuntimeInstanceId,
    pub container_id: ContainerId,
}

#[derive(Debug, PartialEq, Eq)]
// A missing runtime materialization re-creatable purely from persisted identity;
// `unit_needs_write` records whether the Quadlet source must be rewritten first.
pub struct RuntimeRematerialization {
    pub unit_needs_write: bool,
}

#[derive(Debug, PartialEq, Eq)]
// Public exposure drift that must never be repaired automatically; execution
// records the failure evidence and reports the outcome.
pub struct PublicExposureFailure {
    // Materialization state the failure record is compare-and-set against.
    pub expected_state: ExposureMaterializationState,
    pub kind: PublicExposureFailureKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PublicExposureFailureKind {
    RuntimeMissing,
    RuntimeNotHealthy,
}

impl PublicExposureFailureKind {
    // Stable diagnostic code persisted alongside the exposure failure record.
    pub fn code(&self) -> &'static str {
        match self {
            Self::RuntimeMissing => "runtime_missing",
            Self::RuntimeNotHealthy => "runtime_not_healthy",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::RuntimeMissing => "public exposure has no active runtime",
            Self::RuntimeNotHealthy => {
                "public exposure requires a confirmed healthy running runtime"
            }
        }
    }
}

#[derive(Debug)]
// Drift detected after every safe rule was evaluated; reconciliation stops
// instead of guessing.
pub enum ReconciliationDecisionError {
    UnhandledDrift,
    InvalidRouteFragment(InvalidExposureConfigurationVersion),
}

// Classifies the next reconciliation action without touching SQLite, Podman,
// systemd, Caddy, the filesystem, clocks, or randomness.
//
// Precedence mirrors the externally relied-on behavior: converged stopped
// state first, then runtime identity repair, then rematerialization, then
// exposure drift, then the manual-intervention fallbacks.
pub fn decide(
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
    expectations: &ReconciliationExpectations,
) -> Result<ReconciliationDecision, ReconciliationDecisionError> {
    let desired_state = input.desired.application.desired_runtime_state;
    if desired_state == DesiredRuntimeState::Stopped
        && *observation.recorded_container.state() == ObservedRuntimeState::Missing
        && observation.named_container == NamedContainerObservation::Missing
        && observation.caddy_fragment == CaddyFragmentObservation::Missing
    {
        return Ok(ReconciliationDecision::InSync);
    }
    if let Some(decision) = classify_runtime_identity_repair(input, observation, expectations) {
        return Ok(decision);
    }
    if let Some(decision) = classify_runtime_rematerialization(input, observation, expectations) {
        return Ok(decision);
    }
    if let Some(decision) = classify_exposure(input, observation, expectations)? {
        return Ok(decision);
    }
    if desired_state == DesiredRuntimeState::Running {
        return Ok(ReconciliationDecision::RequireManualIntervention(
            "runtime identity or configuration differs from persisted intent".to_owned(),
        ));
    }
    Err(ReconciliationDecisionError::UnhandledDrift)
}

// Repairs only a recreated container whose full identity matches the persisted
// active runtime while the recorded container is gone and no public route exists.
fn classify_runtime_identity_repair(
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
    expectations: &ReconciliationExpectations,
) -> Option<ReconciliationDecision> {
    if input.desired.application.desired_runtime_state != DesiredRuntimeState::Running
        || observation.caddy_fragment != CaddyFragmentObservation::Missing
    {
        return None;
    }
    let active = input.persisted.active.as_ref()?;
    let runtime = active.runtime.as_ref()?;
    let NamedContainerObservation::Present {
        id,
        name,
        image_reference,
        application_label,
        image_digest_label,
        observation: container_observation,
    } = &observation.named_container
    else {
        return None;
    };
    if *observation.recorded_container.state() != ObservedRuntimeState::Missing
        || *container_observation.state() != ObservedRuntimeState::Running
        || name.trim_start_matches('/') != expectations.container_name
        || image_reference != active.release.artifact.reference()
        || application_label.as_deref() != Some(input.desired.application.name.as_str())
        || image_digest_label.as_deref() != Some(active.release.artifact.digest())
        || container_observation.observed_endpoint()
            != Some(runtime.expected_endpoint.socket_addr())
        || observation.quadlet_source
            != (QuadletSourceObservation::Present {
                contents: expectations.canonical_quadlet_contents.clone(),
            })
    {
        return None;
    }
    Some(ReconciliationDecision::RepairRuntime(
        RuntimeIdentityRepair {
            runtime_id: runtime.id.clone(),
            container_id: id.clone(),
        },
    ))
}

// Rematerializes an absent runtime only when nothing contradicts a clean start
// from the persisted identity: no recorded or named container, a missing or
// canonical Quadlet source, and a generated unit that is not running.
fn classify_runtime_rematerialization(
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
    expectations: &ReconciliationExpectations,
) -> Option<ReconciliationDecision> {
    let (Some(_active), Some(_specification)) =
        (&input.persisted.active, &input.persisted.specification)
    else {
        return None;
    };
    let quadlet_is_canonical = observation.quadlet_source
        == (QuadletSourceObservation::Present {
            contents: expectations.canonical_quadlet_contents.clone(),
        });
    let generated_unit_can_start = match &observation.systemd_unit {
        SystemdUnitObservation::Missing => true,
        SystemdUnitObservation::Present { active_state } => active_state != "active",
    };
    if input.desired.application.desired_runtime_state != DesiredRuntimeState::Running
        || *observation.recorded_container.state() != ObservedRuntimeState::Missing
        || observation.named_container != NamedContainerObservation::Missing
        || (observation.quadlet_source != QuadletSourceObservation::Missing
            && !quadlet_is_canonical)
        || !generated_unit_can_start
    {
        return None;
    }
    Some(ReconciliationDecision::RematerializeRuntime(
        RuntimeRematerialization {
            unit_needs_write: !quadlet_is_canonical,
        },
    ))
}

fn classify_exposure(
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
    expectations: &ReconciliationExpectations,
) -> Result<Option<ReconciliationDecision>, ReconciliationDecisionError> {
    let Some(exposure) = &input.desired.exposure else {
        return Ok(None);
    };
    let state = exposure.materialization().state();
    if state == ExposureMaterializationState::Diverged {
        return Ok(Some(ReconciliationDecision::RequireManualIntervention(
            "exposure materialization diverged and requires manual intervention".to_owned(),
        )));
    }
    match exposure.intent().visibility() {
        Visibility::Internal => {
            if observation.caddy_fragment == CaddyFragmentObservation::Missing {
                return Ok(None);
            }
            Ok(Some(ReconciliationDecision::RemoveInternalRoute {
                expected_state: state,
            }))
        }
        Visibility::Public => {
            classify_public_exposure(exposure, input, observation, expectations, state).map(Some)
        }
    }
}

// Public routes are never auto-removed; they are confirmed, materialized, or
// failed explicitly. The validated `ExposureIntent` guarantees a domain, so no
// domain-missing classification exists here.
fn classify_public_exposure(
    exposure: &Exposure,
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
    expectations: &ReconciliationExpectations,
    state: ExposureMaterializationState,
) -> Result<ReconciliationDecision, ReconciliationDecisionError> {
    let Some(active) = &input.persisted.active else {
        return Ok(ReconciliationDecision::RecordPublicExposureFailure(
            PublicExposureFailure {
                expected_state: state,
                kind: PublicExposureFailureKind::RuntimeMissing,
            },
        ));
    };
    let Some(runtime) = &active.runtime else {
        return Ok(ReconciliationDecision::RecordPublicExposureFailure(
            PublicExposureFailure {
                expected_state: state,
                kind: PublicExposureFailureKind::RuntimeMissing,
            },
        ));
    };
    if input.desired.application.desired_runtime_state != DesiredRuntimeState::Running
        || *observation.recorded_container.state() != ObservedRuntimeState::Running
        || observation.recorded_container.observed_endpoint()
            != Some(runtime.expected_endpoint.socket_addr())
    {
        return Ok(ReconciliationDecision::RecordPublicExposureFailure(
            PublicExposureFailure {
                expected_state: state,
                kind: PublicExposureFailureKind::RuntimeNotHealthy,
            },
        ));
    }
    let Some(canonical_fragment) = expectations.canonical_route_fragment.as_ref() else {
        // Unreachable when expectations were built with the same active runtime;
        // refusing to guess keeps the decision total and side-effect free.
        return Err(ReconciliationDecisionError::UnhandledDrift);
    };
    let configuration_version = ExposureConfigurationVersion::new(canonical_fragment)
        .map_err(ReconciliationDecisionError::InvalidRouteFragment)?;
    let route_is_confirmed = exposure
        .materialization()
        .confirmed_route()
        .is_some_and(|route| {
            route.runtime_id() == &runtime.id
                && route.configuration_version() == &configuration_version
        });
    if observation.caddy_fragment
        == (CaddyFragmentObservation::Present {
            contents: canonical_fragment.clone(),
        })
        && state == ExposureMaterializationState::Active
        && route_is_confirmed
    {
        return Ok(ReconciliationDecision::InSync);
    }
    Ok(ReconciliationDecision::MaterializePublicRoute {
        expected_state: state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::application::ApplicationName;
    use crate::domain::deployment::{Deployment, DeploymentLifecycle, DeploymentType};
    use crate::domain::exposure::{
        ConfirmedRoute, ExposureDiagnostic, ExposureIntent, ExposureMaterialization,
    };
    use crate::domain::identity::{ApplicationId, DeploymentId, ReleaseId};
    use crate::domain::release::{OciArtifact, Release};
    use crate::domain::runtime::{
        ContainerPort, ExpectedRuntimeEndpoint, HealthCheckPath, HealthCheckSpecification,
        HealthCheckStatus, RuntimeSpecification, RuntimeState,
    };
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    const APPLICATION_ID: &str = "11111111111111111111111111111111";
    const RELEASE_ID: &str = "22222222222222222222222222222222";
    const DEPLOYMENT_ID: &str = "33333333333333333333333333333333";
    const RUNTIME_ID: &str = "44444444444444444444444444444444";
    const CONTAINER_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const CANONICAL_UNIT: &str = "canonical-unit";
    const CANONICAL_ROUTE: &str = "app.example { reverse_proxy }";

    fn application_name() -> ApplicationName {
        ApplicationName::new("app").unwrap()
    }

    fn endpoint() -> ExpectedRuntimeEndpoint {
        ExpectedRuntimeEndpoint::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 30000))
            .unwrap()
    }

    fn socket_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 30000)
    }

    fn deployment() -> Deployment {
        Deployment {
            id: DeploymentId::from(DEPLOYMENT_ID),
            application_id: ApplicationId::from(APPLICATION_ID),
            release_id: ReleaseId::from(RELEASE_ID),
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
            id: ReleaseId::from(RELEASE_ID),
            application_id: ApplicationId::from(APPLICATION_ID),
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
            id: RuntimeInstanceId::from(RUNTIME_ID),
            application_id: ApplicationId::from(APPLICATION_ID),
            deployment_id: DeploymentId::from(DEPLOYMENT_ID),
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
            application_id: ApplicationId::from(APPLICATION_ID),
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

    fn input(
        desired_state: DesiredRuntimeState,
        exposure: Option<Exposure>,
    ) -> ReconciliationInput {
        ReconciliationInput {
            desired: DesiredState {
                application: Application {
                    id: ApplicationId::from(APPLICATION_ID),
                    system_id: None,
                    name: application_name(),
                    desired_runtime_state: desired_state,
                    active_deployment_id: Some(DeploymentId::from(DEPLOYMENT_ID)),
                    manifest_schema_version: 3,
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
            ApplicationId::from(APPLICATION_ID),
            ExposureIntent::new(Visibility::Internal, None).unwrap(),
            ExposureMaterialization::hydrate(state, Option::<ConfirmedRoute>::None, None).unwrap(),
        )
    }

    fn public_exposure(state: ExposureMaterializationState, confirmed_route: bool) -> Exposure {
        let route = confirmed_route.then(|| {
            ConfirmedRoute::new(
                crate::domain::identity::RuntimeInstanceId::from(RUNTIME_ID),
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
            ApplicationId::from(APPLICATION_ID),
            ExposureIntent::new(
                Visibility::Public,
                Some(crate::domain::exposure::DomainName::new("app.example").unwrap()),
            )
            .unwrap(),
            ExposureMaterialization::hydrate(state, route, diagnostic).unwrap(),
        )
    }

    #[test]
    fn stopped_application_fully_absent_is_in_sync() {
        let input = input(DesiredRuntimeState::Stopped, None);
        let decision = decide(&input, &everything_missing(), &expectations(None)).unwrap();
        assert_eq!(decision, ReconciliationDecision::InSync);
    }

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
                runtime_id: RuntimeInstanceId::from(RUNTIME_ID),
                container_id: ContainerId::from(CONTAINER_ID),
            })
        );
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
