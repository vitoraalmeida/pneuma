use std::fmt;

use std::path::{Component, Path};
use thiserror::Error;

// Git-domain concepts only: how an application's source repository is
// classified and addressed, which manifest path inside a checkout is safe to
// read, and what a full commit identity looks like. No TOML parsing and no
// application lifecycle rules live here; those belong to the manifest boundary
// and the owning entities.

// Classifies a Git location by transport prefix so adapters choose between a
// local filesystem checkout and a remote clone without re-parsing the location.
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

    pub(crate) fn from_location(
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

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid application source")]
pub struct InvalidApplicationSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativeManifestPath(String);

impl RelativeManifestPath {
    // The path must stay inside the checkout: absolute paths, parent traversal,
    // and platform roots would let a manifest location escape the repository.
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

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid relative manifest path `{path}`")]
pub struct InvalidRelativeManifestPath {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents the immutable full commit identifier shared by Git, OCI tags, and Deployments.
pub struct CommitSha(String);

impl CommitSha {
    // Only full lowercase SHA-1s are accepted because the commit is shared as
    // an immutable identity by Git resolution, OCI image tags, and Deployment
    // provenance; abbreviated or uppercase forms would make those links ambiguous.
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

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid commit SHA `{value}`")]
pub struct InvalidCommitSha {
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::{ApplicationSource, CommitSha, RelativeManifestPath, RepositoryKind};

    fn manifest_path() -> RelativeManifestPath {
        RelativeManifestPath::new("deploy/pneuma.toml").expect("test path is valid")
    }

    #[test]
    fn classifies_locations_by_transport_prefix() {
        assert_eq!(
            RepositoryKind::from_location("https://example.test/application.git"),
            RepositoryKind::Remote
        );
        assert_eq!(
            RepositoryKind::from_location("git@example.test:team/application.git"),
            RepositoryKind::Remote
        );
        assert_eq!(
            RepositoryKind::from_location("/srv/checkouts/application"),
            RepositoryKind::Local
        );
        assert_eq!(RepositoryKind::from_location("."), RepositoryKind::Local);
    }

    #[test]
    fn builds_local_and_remote_sources_preserving_the_supplied_fields() {
        let remote = ApplicationSource::new(
            RepositoryKind::Remote,
            "https://example.test/application.git",
            Some("main".to_owned()),
            manifest_path(),
        )
        .expect("remote source is valid");
        assert_eq!(remote.repository_kind(), RepositoryKind::Remote);
        assert_eq!(
            remote.repository_location(),
            "https://example.test/application.git"
        );
        assert_eq!(remote.default_branch(), Some("main"));
        assert_eq!(remote.manifest_path().as_str(), "deploy/pneuma.toml");

        let local = ApplicationSource::new(
            RepositoryKind::Local,
            "/srv/checkouts/application",
            None,
            manifest_path(),
        )
        .expect("local source is valid");
        assert_eq!(local.repository_kind(), RepositoryKind::Local);
        assert_eq!(local.repository_location(), "/srv/checkouts/application");
        assert_eq!(local.default_branch(), None);
    }

    #[test]
    fn rejects_empty_untrimmed_or_kind_mismatched_locations() {
        for (kind, location) in [
            (RepositoryKind::Local, ""),
            (RepositoryKind::Local, "/srv/checkouts/application "),
            (
                RepositoryKind::Local,
                "https://example.test/application.git",
            ),
            (RepositoryKind::Remote, "/srv/checkouts/application"),
            (
                RepositoryKind::Remote,
                " https://example.test/application.git",
            ),
        ] {
            assert!(
                ApplicationSource::new(kind, location, None, manifest_path()).is_err(),
                "{kind:?} with {location:?}"
            );
        }
    }

    #[test]
    fn relative_manifest_paths_stay_relative_and_inside_the_checkout() {
        assert!(RelativeManifestPath::new("pneuma.toml").is_ok());
        assert!(RelativeManifestPath::new("deploy/pneuma.toml").is_ok());
        for invalid in [
            "",
            "/etc/pneuma.toml",
            "../pneuma.toml",
            "deploy/../pneuma.toml",
        ] {
            assert!(RelativeManifestPath::new(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn commit_identities_are_full_lowercase_hex_sha1s() {
        assert!(CommitSha::new(&"0123abcdef".repeat(4)).is_ok());
        for invalid in [
            "",
            "short",
            &"A".repeat(40),
            &format!("{}g", "a".repeat(39)),
            &"a".repeat(41),
        ] {
            assert!(CommitSha::new(invalid).is_err(), "{invalid:?}");
        }
    }
}
