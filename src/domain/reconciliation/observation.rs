//! Observed and expected facts that reconciliation compares: what Podman,
//! systemd, and Caddy expose right now, grouped in
//! [`ReconciliationObservation`], and what SQLite records as desired intent,
//! persisted bookkeeping, and boundary-rendered expectations.

use std::net::SocketAddr;

use crate::domain::application::{Application, ApplicationDeploymentSpecification};
use crate::domain::deployment::Deployment;
use crate::domain::exposure::Exposure;
use crate::domain::release::{OciArtifact, Release};
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
pub(crate) enum NamedContainerObservation {
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

impl NamedContainerObservation {
    // Single owner of the rule deciding whether an observed named container is
    // exactly the runtime Pneuma persisted: running state, stable container
    // name, release artifact reference, application and digest labels, and the
    // reserved endpoint must all agree. Planning and post-effect confirmation
    // ask this one predicate so they can never diverge field by field.
    //
    // Podman reports named containers with a leading slash; trimming happens
    // only here so callers never re-implement the normalization.
    pub(crate) fn matches_expected_runtime(
        &self,
        expected_name: &str,
        artifact: &OciArtifact,
        application_name: &str,
        expected_endpoint: SocketAddr,
    ) -> bool {
        let Self::Present {
            name,
            image_reference,
            application_label,
            image_digest_label,
            observation,
            ..
        } = self
        else {
            return false;
        };
        *observation.state() == ObservedRuntimeState::Running
            && name.trim_start_matches('/') == expected_name
            && image_reference == artifact.reference()
            && application_label.as_deref() == Some(application_name)
            && image_digest_label.as_deref() == Some(artifact.digest())
            && observation.observed_endpoint() == Some(expected_endpoint)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Preserves the source bytes needed to classify a Quadlet as canonical or divergent later.
pub enum QuadletSourceObservation {
    Missing,
    Present { contents: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Retains systemd's generated-unit facts without treating an absent unit as an operational failure.
pub(crate) enum SystemdUnitObservation {
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
pub(crate) struct ReconciliationObservation {
    pub(crate) recorded_container: ContainerObservation,
    pub(crate) named_container: NamedContainerObservation,
    pub(crate) quadlet_source: QuadletSourceObservation,
    pub(crate) systemd_unit: SystemdUnitObservation,
    pub(crate) caddy_fragment: CaddyFragmentObservation,
}

// Boundary-rendered external representations that observed files must match to
// count as canonical. The adapters own the exact bytes; the pure decision only
// compares them against observations.
#[derive(Debug)]
pub(crate) struct ReconciliationExpectations {
    pub(crate) container_name: String,
    pub(crate) canonical_quadlet_contents: String,
    // Some only when a public exposure names a domain and an active runtime endpoint exists.
    pub(crate) canonical_route_fragment: Option<String>,
}
