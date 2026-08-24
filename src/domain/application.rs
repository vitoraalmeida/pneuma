use std::error::Error;
use std::fmt;

use crate::domain::exposure::Visibility;
use crate::domain::identity::{ApplicationId, DeploymentId, SystemId};

// Entity — the invariant authority for the application aggregate — and a fact
// bundle: it survives deployments,
// rollbacks, and Pneuma restarts. Mutations of intent are ID-keyed store
// primitives rather than entity field writes (INV-APP-002), so this type is not
// a mutable command target.
#[derive(Debug, Clone, PartialEq, Eq)]
// Captures durable application identity and persisted runtime intent.
pub struct Application {
    pub id: ApplicationId,
    // None only for rows persisted before Systems existed (migration 0005);
    // every import writes exactly one System (`insert_application` takes `&SystemId`).
    pub system_id: Option<SystemId>,
    pub name: ApplicationName,
    // Operator-requested lifecycle intent (`running`/`stopped`). This is what
    // Pneuma should converge to — deliberately distinct from the observed
    // external state carried by runtime types.
    pub desired_runtime_state: DesiredRuntimeState,
    // The deployment currently serving the application; `None` until a first
    // promotion succeeds. Written only by the guarded atomic activation primitive.
    pub active_deployment_id: Option<DeploymentId>,
    // Immutable copy of the manifest `schema_version` recorded at import time
    // (`insert_application`); never updated, compared, or incremented afterwards.
    // Legacy rows imported before the field carried a value may persist older
    // schema versions (e.g. 1) and are tolerated at hydration.
    pub manifest_schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Read model (projection): catalog fields for list/import/show outputs. It carries
// no invariant authority — every decision or mutation path must load the
// `Application` entity instead of consuming this projection.
pub struct ApplicationSummary {
    pub id: ApplicationId,
    // Same legacy-only None semantics as `Application::system_id`.
    pub system_id: Option<SystemId>,
    pub name: ApplicationName,
    pub repository: Option<String>,
    pub default_branch: Option<String>,
    pub desired_runtime_state: DesiredRuntimeState,
    pub active_deployment_id: Option<DeploymentId>,
    // Same immutable manifest `schema_version` copy as `Application`.
    pub manifest_schema_version: u32,
}

// Catalog name for one application. Validated here once; every later boundary
// re-checks through this constructor instead of re-implementing the grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationName(String);

impl ApplicationName {
    pub fn new(value: &str) -> Result<Self, InvalidApplicationName> {
        if !is_valid_catalog_name(value) {
            return Err(InvalidApplicationName {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApplicationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidApplicationName {
    pub value: String,
}
impl fmt::Display for InvalidApplicationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid catalog name `{}`", self.value)
    }
}
impl Error for InvalidApplicationName {}

use crate::domain::identity::is_valid_catalog_name;

// The two lifecycle intents an operator can request. Kept separate from
// `runtime::RuntimeState` (what is actually deployed) and
// `ObservedRuntimeState` (what the host reports) so desired state can never be
// confused with observed state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesiredRuntimeState {
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Use-case input (persisted fact bundle): the application settings needed to
// activate a deployment, loaded as a whole by deploy/promote/reconciliation
// flows. It is not an entity; intent changes stay in ID-keyed store primitives.
pub struct ApplicationDeploymentSpecification {
    pub application_id: ApplicationId,
    pub application_name: ApplicationName,
    pub runtime: crate::domain::runtime::RuntimeSpecification,
    pub visibility: Visibility,
}

#[cfg(test)]
mod tests {
    use super::ApplicationName;

    #[test]
    fn accepts_catalog_names_within_the_shared_rule() {
        assert!(ApplicationName::new("a").is_ok());
        assert!(ApplicationName::new("personal-site").is_ok());
        let longest_allowed = format!("a{}b", "c".repeat(61));
        assert_eq!(longest_allowed.len(), 63);
        assert!(ApplicationName::new(&longest_allowed).is_ok());
    }

    #[test]
    fn rejects_names_outside_the_shared_rule() {
        let too_long = format!("{}c", "a".repeat(63));
        assert_eq!(too_long.len(), 64);
        for invalid in [
            "",
            "Personal-Site",
            "personal site",
            "personal_site",
            "-site",
            "site-",
            "personal.site",
            too_long.as_str(),
        ] {
            assert!(ApplicationName::new(invalid).is_err(), "{invalid:?}");
        }
    }
}
