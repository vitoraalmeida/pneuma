use std::error::Error;
use std::fmt;
use std::path::{Component, Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryKind {
    Local,
    Remote,
}

impl RepositoryKind {
    // Classifies the Git forms accepted by import and branch resolution.
    pub fn from_location(location: &str) -> Self {
        if location.contains("://") || location.starts_with("git@") {
            Self::Remote
        } else {
            Self::Local
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Couples the persisted Git kind with the only location form valid for that kind.
pub enum ApplicationSource {
    Local {
        repository_path: String,
        default_branch: Option<String>,
        manifest_path: RelativeManifestPath,
    },
    Remote {
        repository_url: String,
        default_branch: Option<String>,
        manifest_path: RelativeManifestPath,
    },
}

impl ApplicationSource {
    pub fn new(
        kind: RepositoryKind,
        location: &str,
        default_branch: Option<String>,
        manifest_path: RelativeManifestPath,
    ) -> Result<Self, InvalidApplicationSource> {
        if location.is_empty()
            || location.trim() != location
            || kind != RepositoryKind::from_location(location)
        {
            return Err(InvalidApplicationSource);
        }
        Ok(match kind {
            RepositoryKind::Local => Self::Local {
                repository_path: location.to_owned(),
                default_branch,
                manifest_path,
            },
            RepositoryKind::Remote => Self::Remote {
                repository_url: location.to_owned(),
                default_branch,
                manifest_path,
            },
        })
    }

    pub fn from_location(
        location: &str,
        default_branch: Option<String>,
        manifest_path: RelativeManifestPath,
    ) -> Result<Self, InvalidApplicationSource> {
        Self::new(
            RepositoryKind::from_location(location),
            location,
            default_branch,
            manifest_path,
        )
    }

    pub fn repository_kind(&self) -> RepositoryKind {
        match self {
            Self::Local { .. } => RepositoryKind::Local,
            Self::Remote { .. } => RepositoryKind::Remote,
        }
    }

    pub fn repository_location(&self) -> &str {
        match self {
            Self::Local {
                repository_path, ..
            } => repository_path,
            Self::Remote { repository_url, .. } => repository_url,
        }
    }

    pub fn default_branch(&self) -> Option<&str> {
        match self {
            Self::Local { default_branch, .. } | Self::Remote { default_branch, .. } => {
                default_branch.as_deref()
            }
        }
    }

    pub fn manifest_path(&self) -> &RelativeManifestPath {
        match self {
            Self::Local { manifest_path, .. } | Self::Remote { manifest_path, .. } => manifest_path,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidApplicationSource;

impl fmt::Display for InvalidApplicationSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid application source")
    }
}

impl Error for InvalidApplicationSource {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativeManifestPath(String);

impl RelativeManifestPath {
    pub fn new(value: &str) -> Result<Self, InvalidRelativeManifestPath> {
        let path = Path::new(value);
        if value.is_empty()
            || path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(InvalidRelativeManifestPath {
                path: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidRelativeManifestPath {
    pub path: String,
}

impl fmt::Display for InvalidRelativeManifestPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid relative manifest path `{}`", self.path)
    }
}

impl Error for InvalidRelativeManifestPath {}

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents the immutable full commit identifier shared by Git, OCI tags, and Deployments.
pub struct CommitSha(String);

impl CommitSha {
    pub fn new(value: &str) -> Result<Self, InvalidCommitSha> {
        if value.len() != 40
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(InvalidCommitSha {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidCommitSha {
    pub value: String,
}

impl fmt::Display for InvalidCommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid commit SHA `{}`", self.value)
    }
}

impl Error for InvalidCommitSha {}
