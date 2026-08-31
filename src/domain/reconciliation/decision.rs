//! The reconciliation decision engine: classification of desired, persisted,
//! and observed facts into the next action for one application.
//!
//! Reading top-down: [`decide`] applies the externally relied-on precedence —
//! converged stopped state first, then runtime identity repair, then
//! rematerialization, then exposure drift, then the manual-intervention
//! fallbacks. Nothing here touches SQLite, Podman, systemd, Caddy, the
//! filesystem, clocks, or randomness.

use thiserror::Error;

use crate::domain::application::DesiredRuntimeState;
use crate::domain::exposure::{
    Exposure, ExposureConfigurationVersion, ExposureMaterializationState,
    InvalidExposureConfigurationVersion, Visibility,
};
use crate::domain::identity::RuntimeInstanceId;
use crate::domain::runtime::{ContainerId, ObservedRuntimeState};

use super::observation::{
    CaddyFragmentObservation, NamedContainerObservation, QuadletSourceObservation,
    ReconciliationExpectations, ReconciliationInput, ReconciliationObservation,
    SystemdUnitObservation,
};

#[derive(Debug, PartialEq, Eq)]
// What reconciliation should do next for one application, decided purely from
// persisted facts, observations, and boundary expectations before any effect.
pub(crate) enum ReconciliationDecision {
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
pub(crate) struct RuntimeIdentityRepair {
    pub(crate) runtime_id: RuntimeInstanceId,
    pub(crate) container_id: ContainerId,
}

#[derive(Debug, PartialEq, Eq)]
// A missing runtime materialization re-creatable purely from persisted identity;
// `unit_needs_write` records whether the Quadlet source must be rewritten first.
pub(crate) struct RuntimeRematerialization {
    pub(crate) unit_needs_write: bool,
}

#[derive(Debug, PartialEq, Eq)]
// Public exposure drift that must never be repaired automatically; execution
// records the failure evidence and reports the outcome.
pub(crate) struct PublicExposureFailure {
    // Materialization state the failure record is compare-and-set against.
    pub(crate) expected_state: ExposureMaterializationState,
    pub(crate) kind: PublicExposureFailureKind,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PublicExposureFailureKind {
    RuntimeMissing,
    RuntimeNotHealthy,
}

impl PublicExposureFailureKind {
    // Stable diagnostic code persisted alongside the exposure failure record.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::RuntimeMissing => "runtime_missing",
            Self::RuntimeNotHealthy => "runtime_not_healthy",
        }
    }

    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::RuntimeMissing => "public exposure has no active runtime",
            Self::RuntimeNotHealthy => {
                "public exposure requires a confirmed healthy running runtime"
            }
        }
    }
}

#[derive(Debug, Error)]
// Drift detected after every safe rule was evaluated; reconciliation stops
// instead of guessing.
pub(crate) enum ReconciliationDecisionError {
    #[error("drift has no automatic repair; manual intervention is required")]
    UnhandledDrift,
    #[error(transparent)]
    InvalidRouteFragment(InvalidExposureConfigurationVersion),
}

// Classifies the next reconciliation action without touching SQLite, Podman,
// systemd, Caddy, the filesystem, clocks, or randomness.
//
// Precedence mirrors the externally relied-on behavior: converged stopped
// state first, then runtime identity repair, then rematerialization, then
// exposure drift, then the manual-intervention fallbacks.
pub(crate) fn decide(
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
        || *observation.recorded_container.state() != ObservedRuntimeState::Missing
    {
        return None;
    }
    let active = input.persisted.active.as_ref()?;
    let runtime = active.runtime.as_ref()?;
    let NamedContainerObservation::Present { id, .. } = &observation.named_container else {
        return None;
    };
    if !observation.named_container.matches_expected_runtime(
        &expectations.container_name,
        &active.release.artifact,
        input.desired.application.name.as_str(),
        runtime.expected_endpoint.socket_addr(),
    ) || !quadlet_source_is_canonical(observation, expectations)
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

// Single owner of the canonicality rule: the observed Quadlet source must equal
// the boundary-rendered canonical bytes exactly, in every classification path
// that would rewrite or keep the unit.
fn quadlet_source_is_canonical(
    observation: &ReconciliationObservation,
    expectations: &ReconciliationExpectations,
) -> bool {
    observation.quadlet_source
        == (QuadletSourceObservation::Present {
            contents: expectations.canonical_quadlet_contents.clone(),
        })
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
    let quadlet_is_canonical = quadlet_source_is_canonical(observation, expectations);
    let generated_unit_can_start = match &observation.systemd_unit {
        SystemdUnitObservation::Missing => true,
        SystemdUnitObservation::Present { active_state } => {
            known_not_running_unit_state(active_state)
        }
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

// Conservative classification of systemd's open-ended active-state vocabulary:
// only the documented not-running states authorize an automatic start. Transient
// or unrecognized states fall through to manual intervention instead of being
// silently adopted as startable.
fn known_not_running_unit_state(active_state: &str) -> bool {
    matches!(active_state, "inactive" | "failed")
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
