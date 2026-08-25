use serde::Deserialize;
use thiserror::Error;

use crate::domain::identity::{ApplicationId, ReleaseId};

const DIGEST_ALGORITHM: &str = "sha256:";
const SHA256_HEX_LENGTH: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
// Names the delivery mechanism a Release artifact is supplied through.
pub enum DeliveryType {
    Oci,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents a validated immutable repository-and-digest OCI artifact identity.
pub struct OciArtifact {
    reference: String,
    repository: OciRepository,
    digest: String,
}

impl OciArtifact {
    // Builds a canonical digest-pinned reference from separately supplied components.
    pub fn new(repository: &str, digest: &str) -> Result<Self, InvalidOciArtifact> {
        Self::parse(&format!("{repository}@{digest}"))
    }

    // Parses only repository@sha256 references so mutable tags never become artifacts.
    pub fn parse(reference: &str) -> Result<Self, InvalidOciArtifact> {
        let Some((repository, digest)) = reference.split_once('@') else {
            return Err(InvalidOciArtifact {
                reference: reference.to_owned(),
            });
        };
        let repository = OciRepository::new(repository).map_err(|_| InvalidOciArtifact {
            reference: reference.to_owned(),
        })?;
        if !is_sha256_digest(digest) {
            return Err(InvalidOciArtifact {
                reference: reference.to_owned(),
            });
        }

        Ok(Self {
            reference: reference.to_owned(),
            repository,
            digest: digest.to_owned(),
        })
    }

    // Returns the canonical digest-pinned reference preserved during validation.
    pub fn reference(&self) -> &str {
        &self.reference
    }

    // Returns the repository portion constrained by artifact validation.
    pub fn repository(&self) -> &str {
        self.repository.as_str()
    }

    // Returns the sha256 digest portion constrained by artifact validation.
    // Deliberately no standalone digest type: the digest is validated exactly
    // once inside `parse` and has no behavior or lifecycle of its own, so it
    // stays an intentional primitive carried by the artifact.
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Identifies an OCI repository without a mutable tag or digest suffix.
pub struct OciRepository(String);

impl OciRepository {
    // Accepts registry host/port plus path components but no tag or digest:
    // mutable identifiers must never masquerade as repository identity.
    pub fn new(repository: &str) -> Result<Self, InvalidOciRepository> {
        if !is_repository(repository) {
            return Err(InvalidOciRepository {
                repository: repository.to_owned(),
            });
        }
        Ok(Self(repository.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Defines the immutable repository boundary allowed for application artifacts.
pub struct DeliverySpecification {
    delivery_type: DeliveryType,
    image_repository: OciRepository,
}

impl DeliverySpecification {
    // Restricted construction: only the manifest/import boundary mints one,
    // after both fields were validated, so no code path can pair an unchecked
    // repository with a delivery type.
    pub(crate) fn new(delivery_type: DeliveryType, image_repository: OciRepository) -> Self {
        Self {
            delivery_type,
            image_repository,
        }
    }
    pub fn delivery_type(&self) -> DeliveryType {
        self.delivery_type
    }
    pub fn image_repository(&self) -> &OciRepository {
        &self.image_repository
    }

    // Cross-object rule: an artifact is deployable only when its repository
    // matches the single repository permitted by this delivery specification.
    pub(crate) fn permits(&self, artifact: &OciArtifact) -> bool {
        self.image_repository.as_str() == artifact.repository()
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid OCI repository `{repository}`")]
pub struct InvalidOciRepository {
    pub repository: String,
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error(
    "image reference `{reference}` must be <repository>@sha256:<64 lowercase hexadecimal characters>"
)]
pub struct InvalidOciArtifact {
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Entity: one immutable artifact reusable by an Application; the invariant
// authority for release identity. A Release never changes after creation; a new
// artifact means a new Release.
pub struct Release {
    pub id: ReleaseId,
    pub application_id: ApplicationId,
    pub artifact: OciArtifact,
    pub created_at: String,
}

// Accepts the restricted repository characters used by artifact validation.
fn is_repository(repository: &str) -> bool {
    if repository.is_empty()
        || repository.trim() != repository
        || repository.contains('@')
        || repository.split('/').any(str::is_empty)
    {
        return false;
    }
    let mut components = repository.split('/');
    let Some(first) = components.next() else {
        return false;
    };
    let remaining = components.collect::<Vec<_>>();
    if first.contains(':') && remaining.is_empty() {
        return false;
    }
    is_repository_component(first, true)
        && remaining
            .iter()
            .all(|component| is_repository_component(component, false))
}

// Only the first component can carry a numeric registry port; a colon in a path is a tag.
fn is_repository_component(component: &str, registry: bool) -> bool {
    let (name, port) = if registry {
        match component.rsplit_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (component, None),
        }
    } else {
        (component, None)
    };
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && port
            .is_none_or(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
}

// Requires a sha256 prefix followed by exactly 64 lowercase hexadecimal characters.
// Shared with the OCI adapter so Podman output uses this single digest authority.
pub(crate) fn is_sha256_digest(digest: &str) -> bool {
    digest
        .strip_prefix(DIGEST_ALGORITHM)
        .is_some_and(|hex| hex.len() == SHA256_HEX_LENGTH && hex.bytes().all(is_lowercase_hex))
}

// Recognizes the lowercase hexadecimal alphabet required by OCI sha256 digests.
fn is_lowercase_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

#[cfg(test)]
mod tests {
    use super::{DeliverySpecification, DeliveryType, OciArtifact, OciRepository};

    fn artifact(repository: &str) -> OciArtifact {
        OciArtifact::new(
            repository,
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("test artifact is valid")
    }

    fn delivery(repository: &str) -> DeliverySpecification {
        DeliverySpecification::new(
            DeliveryType::Oci,
            OciRepository::new(repository).expect("test repository is valid"),
        )
    }

    #[test]
    fn permits_the_exact_configured_repository() {
        assert!(delivery("registry.example/app").permits(&artifact("registry.example/app")));
    }

    #[test]
    fn rejects_foreign_and_prefix_repositories() {
        let allowed = delivery("registry.example/app");
        assert!(!allowed.permits(&artifact("registry.example/other")));
        assert!(!allowed.permits(&artifact("registry.example/app/subpath")));
        assert!(!allowed.permits(&artifact("other.example/app")));
    }
}
