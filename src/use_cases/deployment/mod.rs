//! Deployment use cases, grouped by capability so one deploy can be followed through a
//! small, predictable set of modules.
//!
//! The public commands are re-exported here; every internal step stays private to this
//! module tree. Successful-path reading order:
//!
//! ```text
//! deploy → execute → candidate → activation/promotion → cleanup
//!            \→ failure
//! ```
//!
//! Module responsibilities:
//!
//! - [`deploy`] resolves sources into releases (`deploy_branch`, `deploy_oci`);
//! - [`execute`] runs the release workflow (`deploy_release`);
//! - [`failure`] represents and finalizes failed executions: classification, failure
//!   persistence, candidate cleanup, and recovery-error precedence;
//! - [`candidate`] materializes the candidate runtime (`start_candidate`, registration);
//! - [`activation`] drives public-candidate activation (health, Caddy route, promotion);
//! - [`promotion`] persists candidate confirmation (`promote_internal_candidate`, public
//!   promotion primitives);
//! - [`cleanup`] tracks and retires candidate/previous resources;
//! - [`rollback`] redeploys a historical artifact;
//! - [`transition`] exposes the lifecycle commands (`advance_deployment`, `fail_deployment`);
//! - [`create`] records pending deployments;
//! - [`query`] lists deployment history;
//! - [`progress`] defines the shared progress vocabulary.

mod activation;
mod candidate;
mod cleanup;
mod create;
mod deploy;
mod execute;
mod failure;
mod progress;
mod promotion;
mod query;
mod rollback;
mod transition;

pub use self::candidate::{RegisterCandidateRuntimeError, register_candidate_runtime};
pub(crate) use self::cleanup::cleanup_failed_candidate;
pub use self::create::{
    CreateDeploymentError, create_deployment, create_deployment_with_source_revision,
};
pub use self::deploy::{
    DeployBranchError, DeployOciError, deploy_branch, deploy_branch_with_progress, deploy_oci,
    deploy_oci_with_progress,
};
pub use self::execute::{DeploymentResult, PublicDeploymentConfiguration};
pub use self::failure::DeployReleaseError;
pub use self::progress::{DeploymentProgress, DeploymentStep};
pub use self::promotion::{PromoteInternalCandidateError, promote_internal_candidate};
pub use self::query::list_deployments;
pub use self::rollback::{RollbackError, rollback_deployment};
pub use self::transition::{TransitionDeploymentError, advance_deployment, fail_deployment};
