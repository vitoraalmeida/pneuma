//! Pure comparison of desired and persisted facts against observed external
//! state that yields the next reconciliation action without performing effects.
//!
//! Reading top-down:
//!
//! - [`observation`]: what each authority exposes right now, grouped in one
//!   observation snapshot, plus what SQLite records as desired intent,
//!   persisted bookkeeping, and boundary-rendered expectations;
//! - [`decision`]: the pure classification of those facts into the next
//!   action, refusing unsafe drift as manual intervention.

mod decision;
mod observation;

#[cfg(test)]
mod tests;

pub(crate) use decision::{
    PublicExposureFailure, ReconciliationDecision, ReconciliationDecisionError,
    RuntimeIdentityRepair, RuntimeRematerialization, decide,
};
pub use observation::{
    ActiveRuntime, CaddyFragmentObservation, DesiredState, PersistedState,
    QuadletSourceObservation, ReconciliationInput,
};
pub(crate) use observation::{
    NamedContainerObservation, ReconciliationExpectations, ReconciliationObservation,
    SystemdUnitObservation,
};
