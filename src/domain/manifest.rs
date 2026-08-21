use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::domain::application::ApplicationName;
use crate::domain::delivery::DeliveryType;
use crate::domain::exposure::{DomainName, ExposureIntent, Visibility};
use crate::domain::git::RelativeManifestPath;
use crate::domain::release::OciRepository;
use crate::domain::runtime::{ContainerPort, HealthCheckPath, HealthCheckStatus};
use crate::domain::system::SystemName;

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

#[derive(Debug, Clone, PartialEq, Eq)]
// Carries manifest values after validation into the import workflow.
pub struct ImportSpecification {
    pub schema_version: u32,
    pub system_name: Option<SystemName>,
    pub application_name: ApplicationName,
    pub delivery_type: DeliveryType,
    pub repository: OciRepository,
    pub container_port: ContainerPort,
    pub healthcheck_path: HealthCheckPath,
    pub expected_status: HealthCheckStatus,
    pub exposure: ExposureIntent,
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

impl Manifest {
    // Converts the serde DTO to values whose invariants have been checked once.
    pub fn import_specification(&self) -> Result<ImportSpecification, ManifestError> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchemaVersion {
                found: self.schema_version,
            });
        }
        let system_name = self.system.as_ref().map(|system| SystemName::new(&system.name)).transpose().map_err(|_| ManifestError::InvalidField { field: "system.name", reason: "must be 1-63 lowercase ASCII letters, digits, or hyphens and start and end with a letter or digit" })?;
        let application_name = ApplicationName::new(&self.application.name).map_err(|_| ManifestError::InvalidField { field: "application.name", reason: "must be 1-63 lowercase ASCII letters, digits, or hyphens and start and end with a letter or digit" })?;
        if self.delivery.delivery_type != DeliveryType::Oci {
            return Err(ManifestError::InvalidField {
                field: "delivery.type",
                reason: "must be `oci`",
            });
        }
        let repository =
            OciRepository::new(&self.delivery.image).map_err(|_| ManifestError::InvalidField {
                field: "delivery.image",
                reason: "must be a non-empty OCI repository without surrounding whitespace",
            })?;
        let container_port = ContainerPort::new(self.runtime.container_port).map_err(|_| {
            ManifestError::InvalidField {
                field: "runtime.container_port",
                reason: "must be between 1 and 65535",
            }
        })?;
        let healthcheck_path =
            HealthCheckPath::new(&self.runtime.healthcheck_path).map_err(|_| {
                ManifestError::InvalidField {
                    field: "runtime.healthcheck_path",
                    reason: "must be an absolute HTTP path without whitespace",
                }
            })?;
        let expected_status =
            HealthCheckStatus::new(self.runtime.expected_status).map_err(|_| {
                ManifestError::InvalidField {
                    field: "runtime.expected_status",
                    reason: "must be between 100 and 599",
                }
            })?;
        let domain = self
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
            ExposureIntent::new(self.exposure.default_visibility, domain).map_err(|_| {
                ManifestError::InvalidField {
                    field: "exposure.domain",
                    reason: "is required for public exposure",
                }
            })?;
        Ok(ImportSpecification {
            schema_version: self.schema_version,
            system_name,
            application_name,
            delivery_type: self.delivery.delivery_type,
            repository,
            container_port,
            healthcheck_path,
            expected_status,
            exposure,
        })
    }
}

// Enforces schema and cross-field constraints required before an import is persisted.
fn validate_manifest(manifest: &Manifest) -> Result<(), ManifestError> {
    manifest.import_specification().map(|_| ())
}
