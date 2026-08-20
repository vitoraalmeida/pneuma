pub mod application_store;
pub mod deployment_store;
pub mod release_store;
pub mod runtime_store;
pub mod system_store;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Distinguishes a confirmed compare-and-set write from a concurrent persisted change.
pub enum PersistenceOutcome {
    Updated,
    Stale,
}
