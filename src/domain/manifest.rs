use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::domain::application::{
    ApplicationName, ContainerPort, HealthCheckPath, HealthCheckStatus, RelativeManifestPath,
    SystemName,
};
use crate::domain::delivery::DeliveryType;
use crate::domain::exposure::{DomainName, Visibility};
use crate::domain::release::OciRepository;

const SUPPORTED_SCHEMA_VERSION: u32 = 3;
const MANIFEST_FILE_NAME: &str = "pneuma.toml";

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Represents the complete versioned import specification before persistence.
pub struct Manifest {
    pub schema_version: u32,
    pub system: Option<System>,
    pub application: Application,
    pub delivery: Delivery,
    pub runtime: Runtime,
    pub exposure: Exposure,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Supplies the optional organizational group selected by the manifest.
pub struct System {
    pub name: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Supplies the application identity declared by the repository.
pub struct Application {
    pub name: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Restricts manifest delivery input to its declared mechanism and repository.
pub struct Delivery {
    #[serde(rename = "type")]
    pub delivery_type: DeliveryType,
    pub image: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Defines the container port and HTTP health requirements for activation.
pub struct Runtime {
    pub container_port: u16,
    pub healthcheck_path: String,
    pub expected_status: u16,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Declares route intent and the domain required for public visibility.
pub struct Exposure {
    pub default_visibility: Visibility,
    pub domain: Option<String>,
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
pub fn load_manifest(repository_path: &Path) -> Result<Manifest, ManifestError> {
    load_manifest_at(repository_path, MANIFEST_FILE_NAME)
}

// Reads and validates a caller-selected manifest path under a repository checkout.
pub fn load_manifest_at(
    repository_path: &Path,
    manifest_path: &str,
) -> Result<Manifest, ManifestError> {
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

// Parses TOML and applies all domain validation before returning a manifest.
pub fn parse_manifest(contents: &str) -> Result<Manifest, ManifestError> {
    let manifest = toml::from_str(contents).map_err(|source| ManifestError::Parse { source })?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

// Enforces schema and cross-field constraints required before an import is persisted.
fn validate_manifest(manifest: &Manifest) -> Result<(), ManifestError> {
    if manifest.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchemaVersion {
            found: manifest.schema_version,
        });
    }

    if let Some(system) = &manifest.system {
        if SystemName::new(&system.name).is_err() {
            return invalid_field(
                "system.name",
                "must be 1-63 lowercase ASCII letters, digits, or hyphens and start and end with a letter or digit",
            );
        }
    }

    if ApplicationName::new(&manifest.application.name).is_err() {
        return invalid_field(
            "application.name",
            "must be 1-63 lowercase ASCII letters, digits, or hyphens and start and end with a letter or digit",
        );
    }

    if manifest.delivery.delivery_type != DeliveryType::Oci {
        return invalid_field("delivery.type", "must be `oci`");
    }

    if OciRepository::new(&manifest.delivery.image).is_err() {
        return invalid_field(
            "delivery.image",
            "must be a non-empty OCI repository without surrounding whitespace",
        );
    }

    if ContainerPort::new(manifest.runtime.container_port).is_err() {
        return invalid_field("runtime.container_port", "must be between 1 and 65535");
    }

    if HealthCheckPath::new(&manifest.runtime.healthcheck_path).is_err() {
        return invalid_field(
            "runtime.healthcheck_path",
            "must be an absolute HTTP path without whitespace",
        );
    }

    if HealthCheckStatus::new(manifest.runtime.expected_status).is_err() {
        return invalid_field("runtime.expected_status", "must be between 100 and 599");
    }

    match (
        &manifest.exposure.default_visibility,
        &manifest.exposure.domain,
    ) {
        (Visibility::Public, None) => {
            return invalid_field("exposure.domain", "is required for public exposure");
        }
        (_, Some(domain)) if DomainName::new(domain).is_err() => {
            return invalid_field("exposure.domain", "must be a valid domain name");
        }
        (Visibility::Internal, None) | (Visibility::Internal, Some(_)) => {}
        (Visibility::Public, Some(_)) => {}
    }

    Ok(())
}

// Produces a typed validation failure without constructing an unrelated success value.
fn invalid_field<T>(field: &'static str, reason: &'static str) -> Result<T, ManifestError> {
    // The generic success type lets validation branches return the same error
    // from functions with different `Result` success types; no `T` is created.
    Err(ManifestError::InvalidField { field, reason })
}
