use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

const SUPPORTED_SCHEMA_VERSION: u32 = 1;
const MANIFEST_FILE_NAME: &str = "pneuma.toml";

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub application: Application,
    pub source: Source,
    pub build: Build,
    pub runtime: Runtime,
    pub exposure: Exposure,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Application {
    pub name: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub repository: String,
    pub branch: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub containerfile: PathBuf,
    pub context: PathBuf,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub container_port: u16,
    pub healthcheck_path: String,
    pub expected_status: u16,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Exposure {
    pub default_visibility: Visibility,
    pub domain: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Internal,
    Public,
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

pub fn load_manifest(repository_path: &Path) -> Result<Manifest, ManifestError> {
    let manifest_path = repository_path.join(MANIFEST_FILE_NAME);
    let contents = fs::read_to_string(&manifest_path).map_err(|source| ManifestError::Read {
        path: manifest_path,
        source,
    })?;

    parse_manifest(&contents)
}

pub fn parse_manifest(contents: &str) -> Result<Manifest, ManifestError> {
    let manifest = toml::from_str(contents).map_err(|source| ManifestError::Parse { source })?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &Manifest) -> Result<(), ManifestError> {
    if manifest.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchemaVersion {
            found: manifest.schema_version,
        });
    }

    if !is_valid_application_name(&manifest.application.name) {
        return invalid_field(
            "application.name",
            "must be 1-63 lowercase ASCII letters, digits, or hyphens and start and end with a letter or digit",
        );
    }

    if !is_trimmed_nonempty(&manifest.source.repository) {
        return invalid_field(
            "source.repository",
            "must be non-empty and have no surrounding whitespace",
        );
    }

    if !is_trimmed_nonempty(&manifest.source.branch)
        || manifest.source.branch.chars().any(char::is_whitespace)
    {
        return invalid_field("source.branch", "must be a non-empty branch name");
    }

    validate_relative_path("build.containerfile", &manifest.build.containerfile)?;
    validate_relative_path("build.context", &manifest.build.context)?;

    if manifest.runtime.container_port == 0 {
        return invalid_field("runtime.container_port", "must be between 1 and 65535");
    }

    if !manifest.runtime.healthcheck_path.starts_with('/')
        || manifest
            .runtime
            .healthcheck_path
            .chars()
            .any(char::is_whitespace)
    {
        return invalid_field(
            "runtime.healthcheck_path",
            "must be an absolute HTTP path without whitespace",
        );
    }

    if !(100..=599).contains(&manifest.runtime.expected_status) {
        return invalid_field("runtime.expected_status", "must be between 100 and 599");
    }

    match (
        &manifest.exposure.default_visibility,
        &manifest.exposure.domain,
    ) {
        (Visibility::Public, None) => {
            return invalid_field("exposure.domain", "is required for public exposure");
        }
        (_, Some(domain)) if !is_valid_domain(domain) => {
            return invalid_field("exposure.domain", "must be a valid domain name");
        }
        (Visibility::Internal, None) | (Visibility::Internal, Some(_)) => {}
        (Visibility::Public, Some(_)) => {}
    }

    Ok(())
}

fn validate_relative_path(field: &'static str, path: &Path) -> Result<(), ManifestError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return invalid_field(field, "must be a relative path confined to the checkout");
    }

    Ok(())
}

fn is_valid_application_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

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

fn invalid_field<T>(field: &'static str, reason: &'static str) -> Result<T, ManifestError> {
    // The generic success type lets validation branches return the same error
    // from functions with different `Result` success types; no `T` is created.
    Err(ManifestError::InvalidField { field, reason })
}
