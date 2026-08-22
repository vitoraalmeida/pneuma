use crate::domain::application::{Application, ApplicationDeploymentSpecification};
use crate::domain::deployment::Deployment;
use crate::domain::exposure::Exposure;
use crate::domain::release::Release;
use crate::domain::runtime::{ContainerId, ContainerObservation, RuntimeInstance};

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
