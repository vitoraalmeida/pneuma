use std::fmt;

use std::path::{Component, Path};
use thiserror::Error;

// Git-domain concepts only: how an application's remote source repository is
// addressed, which manifest path inside a checkout is safe to read, and what a
// full commit identity looks like. No TOML parsing and no application
// lifecycle rules live here; those belong to the manifest boundary and the
// owning entities.

#[derive(Debug, Clone, PartialEq, Eq)]
// The one supported source shape: a validated remote Git repository with its
// checkout defaults. Local paths are not a supported import source.
pub struct ApplicationSource {
    repository_url: String,
    default_branch: Option<String>,
    manifest_path: RelativeManifestPath,
}

impl ApplicationSource {
    pub fn new(
        repository_url: &str,
        default_branch: Option<String>,
        manifest_path: RelativeManifestPath,
    ) -> Result<Self, InvalidApplicationSource> {
        if !is_remote_git_location(repository_url) {
            return Err(InvalidApplicationSource {
                repository_url: repository_url.to_owned(),
            });
        }
        Ok(Self {
            repository_url: repository_url.to_owned(),
            default_branch,
            manifest_path,
        })
    }

    pub fn repository_url(&self) -> &str {
        &self.repository_url
    }

    pub fn default_branch(&self) -> Option<&str> {
        self.default_branch.as_deref()
    }

    pub fn manifest_path(&self) -> &RelativeManifestPath {
        &self.manifest_path
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid application source `{repository_url}`: expected a remote Git URL")]
pub struct InvalidApplicationSource {
    pub repository_url: String,
}

// Classifies the Git forms accepted by import and branch resolution: transport
// URLs and scp-like `git@host:path` forms are remote; everything else
// (including local paths) is not a supported source.
pub fn is_remote_git_location(location: &str) -> bool {
    !location.is_empty()
        && location.trim() == location
        && (location.contains("://") || location.starts_with("git@"))
}

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
    use super::{ApplicationSource, CommitSha, RelativeManifestPath, is_remote_git_location};

    fn manifest_path() -> RelativeManifestPath {
        RelativeManifestPath::new("deploy/pneuma.toml").expect("test path is valid")
    }

    #[test]
    fn classifies_locations_by_transport_prefix() {
        assert!(is_remote_git_location(
            "https://example.test/application.git"
        ));
        assert!(is_remote_git_location(
            "git@example.test:team/application.git"
        ));
        assert!(!is_remote_git_location("/srv/checkouts/application"));
        assert!(!is_remote_git_location("."));
        assert!(!is_remote_git_location(""));
        assert!(!is_remote_git_location(
            " https://example.test/application.git"
        ));
    }

    #[test]
    fn builds_a_remote_source_preserving_the_supplied_fields() {
        let source = ApplicationSource::new(
            "https://example.test/application.git",
            Some("main".to_owned()),
            manifest_path(),
        )
        .expect("remote source is valid");
        assert_eq!(
            source.repository_url(),
            "https://example.test/application.git"
        );
        assert_eq!(source.default_branch(), Some("main"));
        assert_eq!(source.manifest_path().as_str(), "deploy/pneuma.toml");
    }

    #[test]
    fn rejects_local_paths_and_untrimmed_locations() {
        for location in [
            "",
            "/srv/checkouts/application",
            "/srv/checkouts/application ",
            ".",
            "git@example.test:team/application.git ",
        ] {
            let error = ApplicationSource::new(location, None, manifest_path()).unwrap_err();
            assert_eq!(error.repository_url, location, "{location:?}");
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
