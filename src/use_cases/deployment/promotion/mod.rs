//! The two candidate promotion workflows. Internal candidates are health-checked and
//! then promoted inside one immediate transaction; public candidates additionally
//! coordinate Caddy route state across SQLite transactions before their promotion is
//! confirmed.
mod internal;
mod public;

pub use self::internal::{PromoteInternalCandidateError, promote_internal_candidate};
pub(crate) use self::public::{
    PromotePublicCandidateError, begin_public_exposure, promote_public_candidate,
    record_public_exposure_failure,
};
