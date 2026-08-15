use std::error::Error;
use std::fmt;

const DIGEST_ALGORITHM: &str = "sha256:";
const SHA256_HEX_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents a validated immutable repository-and-digest OCI artifact identity.
pub struct OciArtifact {
    reference: String,
    repository: String,
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
        if !is_repository(repository) || !is_sha256_digest(digest) {
            return Err(InvalidOciArtifact {
                reference: reference.to_owned(),
            });
        }

        Ok(Self {
            reference: reference.to_owned(),
            repository: repository.to_owned(),
            digest: digest.to_owned(),
        })
    }

    // Revalidates persisted components to detect inconsistent historical rows.
    pub(crate) fn from_persisted(
        reference: &str,
        repository: &str,
        digest: &str,
    ) -> Result<Self, InvalidOciArtifact> {
        let artifact = Self::parse(reference)?;
        if artifact.repository != repository || artifact.digest != digest {
            return Err(InvalidOciArtifact {
                reference: reference.to_owned(),
            });
        }
        Ok(artifact)
    }

    // Returns the canonical digest-pinned reference preserved during validation.
    pub fn reference(&self) -> &str {
        &self.reference
    }

    // Returns the repository portion constrained by artifact validation.
    pub fn repository(&self) -> &str {
        &self.repository
    }

    // Returns the sha256 digest portion constrained by artifact validation.
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidOciArtifact {
    pub reference: String,
}

impl fmt::Display for InvalidOciArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "image reference `{}` must be <repository>@sha256:<64 lowercase hexadecimal characters>",
            self.reference
        )
    }
}

impl Error for InvalidOciArtifact {}

#[derive(Debug, Clone, PartialEq, Eq)]
// Records a reusable immutable artifact associated with one Application.
pub struct Release {
    pub id: String,
    pub application_id: String,
    pub artifact: OciArtifact,
    pub created_at: String,
}

// Accepts the restricted repository characters used by artifact validation.
fn is_repository(repository: &str) -> bool {
    !repository.is_empty()
        && repository.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b'_' | b'-' | b':')
        })
}

// Requires a sha256 prefix followed by exactly 64 lowercase hexadecimal characters.
fn is_sha256_digest(digest: &str) -> bool {
    digest
        .strip_prefix(DIGEST_ALGORITHM)
        .is_some_and(|hex| hex.len() == SHA256_HEX_LENGTH && hex.bytes().all(is_lowercase_hex))
}

// Recognizes the lowercase hexadecimal alphabet required by OCI sha256 digests.
fn is_lowercase_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}
