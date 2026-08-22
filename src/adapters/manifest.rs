use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::domain::application::ApplicationName;
use crate::domain::exposure::{DomainName, ExposureIntent, Visibility};
use crate::domain::git::RelativeManifestPath;
use crate::domain::manifest::ImportSpecification;
use crate::domain::release::{DeliveryType, OciRepository};
use crate::domain::runtime::{ContainerPort, HealthCheckPath, HealthCheckStatus};
use crate::domain::system::SystemName;

const SUPPORTED_SCHEMA_VERSION: u32 = 3;
const MANIFEST_FILE_NAME: &str = "pneuma.toml";

// The structs below are the external TOML representation of a manifest. They
// are private adapter details: nothing outside this module may depend on the
// file schema, and they must never leak into the domain as entities.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDocument {
    schema_version: u32,
    system: Option<SystemSection>,
    application: ApplicationSection,
    delivery: DeliverySection,
    runtime: RuntimeSection,
    exposure: ExposureSection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemSection {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationSection {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliverySection {
    #[serde(rename = "type")]
    delivery_type: DeliveryField,
    image: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeliveryField {
    Oci,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeSection {
    container_port: u16,
    healthcheck_path: String,
    expected_status: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExposureSection {
    default_visibility: Visibility,
    domain: Option<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        source: toml::de::Error,
    },
    UnsupportedSchemaVersion {
        found: u32,
    },
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
}

impl fmt::Display for ManifestError {
    // `Error` requires `Display`; this is the user-facing message shown at the
    // manifest boundary, with enough context to identify the failed operation.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "failed to read manifest at {}: {source}",
                    path.display()
                )
            }
            Self::Parse { source } => write!(formatter, "invalid manifest TOML: {source}"),
            Self::UnsupportedSchemaVersion { found } => write!(
                formatter,
                "unsupported schema_version {found}; expected {SUPPORTED_SCHEMA_VERSION}"
            ),
            Self::InvalidField { field, reason } => {
                write!(formatter, "invalid manifest field `{field}`: {reason}")
            }
        }
    }
}

impl Error for ManifestError {
    // Exposing wrapped library errors lets reporters traverse the cause chain.
    // Validation errors originate in Pneuma, so they have no deeper source.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source } => Some(source),
            Self::UnsupportedSchemaVersion { .. } | Self::InvalidField { .. } => None,
        }
    }
}

// Loads the manifest from Pneuma's fixed repository-relative filename.
pub fn load_manifest(repository_path: &Path) -> Result<ImportSpecification, ManifestError> {
    load_manifest_at(repository_path, MANIFEST_FILE_NAME)
}

// Reads and validates a caller-selected manifest path under a repository checkout.
pub fn load_manifest_at(
    repository_path: &Path,
    manifest_path: &str,
) -> Result<ImportSpecification, ManifestError> {
    let manifest_path =
        RelativeManifestPath::new(manifest_path).map_err(|_| ManifestError::InvalidField {
            field: "manifest_path",
            reason: "must be a relative path within the checkout",
        })?;
    let manifest_path = repository_path.join(manifest_path.as_str());
    let contents = fs::read_to_string(&manifest_path).map_err(|source| ManifestError::Read {
        path: manifest_path,
        source,
    })?;

    parse_manifest(&contents)
}

// Parses TOML, applies all validation, and converts every field into its
// validated domain value in one boundary step.
pub fn parse_manifest(contents: &str) -> Result<ImportSpecification, ManifestError> {
    let document = toml::from_str::<ManifestDocument>(contents)
        .map_err(|source| ManifestError::Parse { source })?;
    import_specification(&document)
}

// Converts the external document into values whose invariants have been checked once.
fn import_specification(document: &ManifestDocument) -> Result<ImportSpecification, ManifestError> {
    if document.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchemaVersion {
            found: document.schema_version,
        });
    }
    let system_name = document
        .system
        .as_ref()
        .map(|system| SystemName::new(&system.name))
        .transpose()
        .map_err(|_| ManifestError::InvalidField {
            field: "system.name",
            reason: "must be 1-63 lowercase ASCII letters, digits, or hyphens and start and end with a letter or digit",
        })?;
    let application_name =
        ApplicationName::new(&document.application.name).map_err(|_| {
            ManifestError::InvalidField {
                field: "application.name",
                reason: "must be 1-63 lowercase ASCII letters, digits, or hyphens and start and end with a letter or digit",
            }
        })?;
    let repository =
        OciRepository::new(&document.delivery.image).map_err(|_| ManifestError::InvalidField {
            field: "delivery.image",
            reason: "must be a non-empty OCI repository without surrounding whitespace",
        })?;
    let delivery_type = match document.delivery.delivery_type {
        DeliveryField::Oci => DeliveryType::Oci,
    };
    let container_port = ContainerPort::new(document.runtime.container_port).map_err(|_| {
        ManifestError::InvalidField {
            field: "runtime.container_port",
            reason: "must be between 1 and 65535",
        }
    })?;
    let healthcheck_path =
        HealthCheckPath::new(&document.runtime.healthcheck_path).map_err(|_| {
            ManifestError::InvalidField {
                field: "runtime.healthcheck_path",
                reason: "must be an absolute HTTP path without whitespace",
            }
        })?;
    let expected_status =
        HealthCheckStatus::new(document.runtime.expected_status).map_err(|_| {
            ManifestError::InvalidField {
                field: "runtime.expected_status",
                reason: "must be between 100 and 599",
            }
        })?;
    let domain = document
        .exposure
        .domain
        .as_deref()
        .map(DomainName::new)
        .transpose()
        .map_err(|_| ManifestError::InvalidField {
            field: "exposure.domain",
            reason: "must be a valid domain name",
        })?;
    let exposure =
        ExposureIntent::new(document.exposure.default_visibility, domain).map_err(|_| {
            ManifestError::InvalidField {
                field: "exposure.domain",
                reason: "is required for public exposure",
            }
        })?;
    Ok(ImportSpecification {
        schema_version: document.schema_version,
        system_name,
        application_name,
        delivery_type,
        repository,
        container_port,
        healthcheck_path,
        expected_status,
        exposure,
    })
}
