use std::error::Error;
use std::fmt;

const DIGEST_ALGORITHM: &str = "sha256:";
const SHA256_HEX_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciArtifact {
    reference: String,
    repository: String,
    digest: String,
}

impl OciArtifact {
    pub fn new(repository: &str, digest: &str) -> Result<Self, InvalidOciArtifact> {
        Self::parse(&format!("{repository}@{digest}"))
    }

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

    pub fn reference(&self) -> &str {
        &self.reference
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

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
pub struct Release {
    pub id: String,
    pub application_id: String,
    pub artifact: OciArtifact,
    pub created_at: String,
}

fn is_repository(repository: &str) -> bool {
    !repository.is_empty()
        && repository.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b'_' | b'-' | b':')
        })
}

fn is_sha256_digest(digest: &str) -> bool {
    digest
        .strip_prefix(DIGEST_ALGORITHM)
        .is_some_and(|hex| hex.len() == SHA256_HEX_LENGTH && hex.bytes().all(is_lowercase_hex))
}

fn is_lowercase_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}
