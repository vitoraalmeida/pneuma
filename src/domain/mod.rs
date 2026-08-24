//! Pure domain layer of Pneuma: value objects, entities, state transitions,
//! and reconciliation policy.
//!
//! Nothing here may depend on SQLite, Podman, systemd, Caddy, Git mechanics,
//! the network, the filesystem, clocks, randomness, or the CLI. Each module
//! owns one domain concept:
//!
//! - `identity`: opaque per-entity identifier types and the shared catalog-name rule;
//! - `application`: durable identity plus desired runtime intent;
//! - `system`: organizational grouping of applications;
//! - `release`: immutable digest-pinned artifacts and delivery rules;
//! - `deployment`: activation attempts and the single legal transition table;
//! - `runtime`: concrete runtime materializations, endpoints, and health contracts;
//! - `exposure`: visibility intent and confirmed public-route evidence;
//! - `git`: source locations, checkout-safe manifest paths, commit identities;
//! - `manifest`: the validated use-case input produced at the TOML boundary;
//! - `reconciliation`: pure comparison of desired/persisted facts against
//!   observations that yields the next action without performing effects.

pub mod application;
pub mod deployment;
pub mod exposure;
pub mod git;
pub mod identity;
pub mod manifest;
pub mod reconciliation;
pub mod release;
pub mod runtime;
pub mod system;
