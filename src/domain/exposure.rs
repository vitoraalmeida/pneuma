use serde::Deserialize;
use std::error::Error;
use std::fmt;

use crate::domain::identity::{ApplicationId, RuntimeInstanceId};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Internal,
    Public,
}

impl Visibility {
    // Serializes the requested route visibility stored as application intent.
    pub fn database_value(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Public => "public",
        }
    }

    // Rejects persisted visibility values outside the supported intent set.
    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "internal" => Some(Self::Internal),
            "public" => Some(Self::Public),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExposureMaterializationState {
    NotMaterialized,
    Applying,
    Active,
    Removing,
    Failed,
    Diverged,
}

impl ExposureMaterializationState {
    // Serializes the last confirmed Caddy materialization state.
    pub(crate) fn database_value(self) -> &'static str {
        match self {
            Self::NotMaterialized => "not_materialized",
            Self::Applying => "applying",
            Self::Active => "active",
            Self::Removing => "removing",
            Self::Failed => "failed",
            Self::Diverged => "diverged",
        }
    }

    // Rejects persisted route states outside the exposure lifecycle.
    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "not_materialized" => Some(Self::NotMaterialized),
            "applying" => Some(Self::Applying),
            "active" => Some(Self::Active),
            "removing" => Some(Self::Removing),
            "failed" => Some(Self::Failed),
            "diverged" => Some(Self::Diverged),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Tracks visibility intent separately from the confirmed Caddy route state.
pub struct Exposure {
    pub application_id: ApplicationId,
    pub desired_visibility: Visibility,
    pub domain: Option<DomainName>,
    pub active_runtime_id: Option<RuntimeInstanceId>,
    pub materialization_state: ExposureMaterializationState,
    pub configuration_version: Option<String>,
    pub last_materialized_at: Option<String>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainName(String);

impl DomainName {
    pub fn new(value: &str) -> Result<Self, InvalidDomainName> {
        if !is_valid_domain(value) {
            return Err(InvalidDomainName {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidDomainName {
    pub value: String,
}
impl fmt::Display for InvalidDomainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid domain name `{}`", self.value)
    }
}
impl Error for InvalidDomainName {}

// Validates an ASCII DNS name within whole-domain and per-label limits.
pub(crate) fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 || !domain.is_ascii() {
        return false;
    }

    domain.split('.').all(is_valid_domain_label)
}

// Enforces DNS label boundaries while permitting only alphanumerics and interior hyphens.
fn is_valid_domain_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}
