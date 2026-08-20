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
// Couples route visibility with the domain requirement that only public intent needs one.
pub enum ExposureIntent {
    Internal { domain: Option<DomainName> },
    Public { domain: DomainName },
}

impl ExposureIntent {
    pub fn new(
        visibility: Visibility,
        domain: Option<DomainName>,
    ) -> Result<Self, InvalidExposure> {
        match (visibility, domain) {
            (Visibility::Internal, domain) => Ok(Self::Internal { domain }),
            (Visibility::Public, Some(domain)) => Ok(Self::Public { domain }),
            (Visibility::Public, None) => {
                Err(InvalidExposure::new("public visibility requires a domain"))
            }
        }
    }

    pub fn visibility(&self) -> Visibility {
        match self {
            Self::Internal { .. } => Visibility::Internal,
            Self::Public { .. } => Visibility::Public,
        }
    }

    pub fn domain(&self) -> Option<&DomainName> {
        match self {
            Self::Internal { domain } => domain.as_ref(),
            Self::Public { domain } => Some(domain),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposureConfigurationVersion(String);

impl ExposureConfigurationVersion {
    pub fn new(value: &str) -> Result<Self, InvalidExposureConfigurationVersion> {
        if value.trim().is_empty() {
            return Err(InvalidExposureConfigurationVersion {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Identifies a Caddy route that was confirmed for a concrete runtime.
pub struct ConfirmedRoute {
    runtime_id: RuntimeInstanceId,
    configuration_version: ExposureConfigurationVersion,
    materialized_at: String,
}

impl ConfirmedRoute {
    pub fn new(
        runtime_id: RuntimeInstanceId,
        configuration_version: ExposureConfigurationVersion,
        materialized_at: String,
    ) -> Result<Self, InvalidExposure> {
        if materialized_at.is_empty() || materialized_at.trim() != materialized_at {
            return Err(InvalidExposure::new(
                "confirmed route requires a materialized timestamp",
            ));
        }
        Ok(Self {
            runtime_id,
            configuration_version,
            materialized_at,
        })
    }

    pub fn runtime_id(&self) -> &RuntimeInstanceId {
        &self.runtime_id
    }

    pub fn configuration_version(&self) -> &ExposureConfigurationVersion {
        &self.configuration_version
    }

    pub fn materialized_at(&self) -> &str {
        &self.materialized_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposureDiagnostic {
    code: String,
    message: String,
}

impl ExposureDiagnostic {
    pub fn new(code: &str, message: &str) -> Result<Self, InvalidExposureDiagnostic> {
        if !is_trimmed_nonempty(code) || !is_trimmed_nonempty(message) {
            return Err(InvalidExposureDiagnostic);
        }
        Ok(Self {
            code: code.to_owned(),
            message: message.to_owned(),
        })
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents the only valid combinations of confirmed route evidence and diagnostics.
pub enum ExposureMaterialization {
    NotMaterialized,
    Applying {
        confirmed_route: Option<ConfirmedRoute>,
    },
    Active {
        confirmed_route: ConfirmedRoute,
    },
    Removing {
        confirmed_route: Option<ConfirmedRoute>,
    },
    Failed {
        confirmed_route: Option<ConfirmedRoute>,
        diagnostic: ExposureDiagnostic,
    },
    Diverged {
        confirmed_route: Option<ConfirmedRoute>,
        diagnostic: ExposureDiagnostic,
    },
}

impl ExposureMaterialization {
    pub fn state(&self) -> ExposureMaterializationState {
        match self {
            Self::NotMaterialized => ExposureMaterializationState::NotMaterialized,
            Self::Applying { .. } => ExposureMaterializationState::Applying,
            Self::Active { .. } => ExposureMaterializationState::Active,
            Self::Removing { .. } => ExposureMaterializationState::Removing,
            Self::Failed { .. } => ExposureMaterializationState::Failed,
            Self::Diverged { .. } => ExposureMaterializationState::Diverged,
        }
    }

    pub fn confirmed_route(&self) -> Option<&ConfirmedRoute> {
        match self {
            Self::NotMaterialized => None,
            Self::Applying { confirmed_route }
            | Self::Removing { confirmed_route }
            | Self::Failed {
                confirmed_route, ..
            }
            | Self::Diverged {
                confirmed_route, ..
            } => confirmed_route.as_ref(),
            Self::Active { confirmed_route } => Some(confirmed_route),
        }
    }

    pub fn diagnostic(&self) -> Option<&ExposureDiagnostic> {
        match self {
            Self::Failed { diagnostic, .. } | Self::Diverged { diagnostic, .. } => Some(diagnostic),
            Self::NotMaterialized
            | Self::Applying { .. }
            | Self::Active { .. }
            | Self::Removing { .. } => None,
        }
    }

    pub fn hydrate(
        state: ExposureMaterializationState,
        confirmed_route: Option<ConfirmedRoute>,
        diagnostic: Option<ExposureDiagnostic>,
    ) -> Result<Self, InvalidExposure> {
        match (state, confirmed_route, diagnostic) {
            (ExposureMaterializationState::NotMaterialized, None, None) => {
                Ok(Self::NotMaterialized)
            }
            (ExposureMaterializationState::Applying, confirmed_route, None) => {
                Ok(Self::Applying { confirmed_route })
            }
            (ExposureMaterializationState::Active, Some(confirmed_route), None) => {
                Ok(Self::Active { confirmed_route })
            }
            (ExposureMaterializationState::Removing, confirmed_route, None) => {
                Ok(Self::Removing { confirmed_route })
            }
            (ExposureMaterializationState::Failed, confirmed_route, Some(diagnostic)) => {
                Ok(Self::Failed {
                    confirmed_route,
                    diagnostic,
                })
            }
            (ExposureMaterializationState::Diverged, confirmed_route, Some(diagnostic)) => {
                Ok(Self::Diverged {
                    confirmed_route,
                    diagnostic,
                })
            }
            (state, _, _) => Err(InvalidExposure::new(&format!(
                "invalid evidence for {}",
                state.database_value()
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Tracks visibility intent separately from confirmed Caddy materialization evidence.
pub struct Exposure {
    pub application_id: ApplicationId,
    intent: ExposureIntent,
    materialization: ExposureMaterialization,
}

impl Exposure {
    pub fn new(
        application_id: ApplicationId,
        intent: ExposureIntent,
        materialization: ExposureMaterialization,
    ) -> Self {
        Self {
            application_id,
            intent,
            materialization,
        }
    }

    pub fn intent(&self) -> &ExposureIntent {
        &self.intent
    }

    pub fn materialization(&self) -> &ExposureMaterialization {
        &self.materialization
    }
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

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidExposureConfigurationVersion {
    pub value: String,
}
impl fmt::Display for InvalidExposureConfigurationVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid exposure configuration version `{}`", self.value)
    }
}
impl Error for InvalidExposureConfigurationVersion {}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidExposureDiagnostic;
impl fmt::Display for InvalidExposureDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("exposure diagnostic code and message must be trimmed and non-empty")
    }
}
impl Error for InvalidExposureDiagnostic {}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidExposure {
    pub reason: String,
}
impl InvalidExposure {
    fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_owned(),
        }
    }
}
impl fmt::Display for InvalidExposure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}
impl Error for InvalidExposure {}

fn is_trimmed_nonempty(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
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
