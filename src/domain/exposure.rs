use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Internal,
    Public,
}

impl Visibility {
    pub fn database_value(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Public => "public",
        }
    }

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
pub struct Exposure {
    pub application_id: String,
    pub desired_visibility: Visibility,
    pub domain: Option<String>,
    pub active_runtime_id: Option<String>,
    pub materialization_state: ExposureMaterializationState,
    pub configuration_version: Option<String>,
    pub last_materialized_at: Option<String>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
}

pub(crate) fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 || !domain.is_ascii() {
        return false;
    }

    domain.split('.').all(is_valid_domain_label)
}

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
