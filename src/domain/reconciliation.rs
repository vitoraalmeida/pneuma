use crate::domain::application::{Application, ApplicationDeploymentSpecification};
use crate::domain::deployment::Deployment;
use crate::domain::exposure::Exposure;
use crate::domain::identity::ContainerId;
use crate::domain::release::Release;
use crate::domain::runtime::{ContainerObservation, RuntimeInstance};

// Collects the persisted authorities needed to classify reconciliation without retaining a SQLite transaction.
#[derive(Debug)]
pub struct ReconciliationInput {
    pub application: Application,
    pub blocking_deployment: Option<Deployment>,
    pub active: Option<ActiveRuntime>,
    pub exposure: Option<Exposure>,
    pub specification: Option<ApplicationDeploymentSpecification>,
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
// Captures the read-only external facts needed by future reconciliation classification.
pub struct ReconciliationObservation {
    pub recorded_container: ContainerObservation,
    pub named_container: NamedContainerObservation,
    pub quadlet_source: QuadletSourceObservation,
    pub systemd_unit: SystemdUnitObservation,
    pub caddy_fragment: CaddyFragmentObservation,
}
