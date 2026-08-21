use std::error::Error;
use std::fmt;
use std::io;
use std::process::Command;

use crate::domain::git::CommitSha;
use crate::domain::release::{InvalidOciArtifact, OciArtifact, OciRepository};

const DIGEST_ALGORITHM: &str = "sha256:";
const SHA256_HEX_LENGTH: usize = 64;

#[derive(Debug, PartialEq, Eq)]
// Represents an OCI artifact that Podman pulled and verified against its immutable digest.
pub struct PulledImage {
    pub artifact: OciArtifact,
}

#[derive(Debug)]
pub enum PullImageError {
    InvalidReference {
        source: InvalidOciArtifact,
    },
    Execute {
        operation: &'static str,
        source: io::Error,
    },
    Pull {
        reference: String,
        stdout: String,
        stderr: String,
    },
    Inspect {
        reference: String,
        stdout: String,
        stderr: String,
    },
    InvalidInspectOutput {
        reference: String,
        output: String,
    },
    DigestMismatch {
        reference: String,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for PullImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReference { source } => source.fmt(formatter),
            Self::Execute { operation, source } => {
                write!(
                    formatter,
                    "failed to execute Podman while {operation}: {source}"
                )
            }
            Self::Pull {
                reference,
                stdout,
                stderr,
            } => write!(
                formatter,
                "failed to pull image `{reference}` with Podman: {}",
                diagnostic(stdout, stderr)
            ),
            Self::Inspect {
                reference,
                stdout,
                stderr,
            } => write!(
                formatter,
                "failed to inspect image `{reference}` with Podman: {}",
                diagnostic(stdout, stderr)
            ),
            Self::InvalidInspectOutput { reference, output } => write!(
                formatter,
                "Podman returned an invalid digest while inspecting image `{reference}`: {output}"
            ),
            Self::DigestMismatch {
                reference,
                expected,
                actual,
            } => write!(
                formatter,
                "Podman inspected image `{reference}` as `{actual}`, expected `{expected}`"
            ),
        }
    }
}

impl Error for PullImageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidReference { source } => Some(source),
            Self::Execute { source, .. } => Some(source),
            Self::Pull { .. }
            | Self::Inspect { .. }
            | Self::InvalidInspectOutput { .. }
            | Self::DigestMismatch { .. } => None,
        }
    }
}

// Pulls a digest-pinned artifact and confirms Podman resolved exactly that digest.
pub fn pull_image(reference: &str) -> Result<PulledImage, PullImageError> {
    let artifact = OciArtifact::parse(reference)
        .map_err(|source| PullImageError::InvalidReference { source })?;
    let pull = Command::new("podman")
        .args(["pull", artifact.reference()])
        .output()
        .map_err(|source| PullImageError::Execute {
            operation: "pulling an image",
            source,
        })?;
    let pull_stdout = String::from_utf8_lossy(&pull.stdout).into_owned();
    let pull_stderr = String::from_utf8_lossy(&pull.stderr).into_owned();
    if !pull.status.success() {
        return Err(PullImageError::Pull {
            reference: artifact.reference().to_owned(),
            stdout: pull_stdout,
            stderr: pull_stderr,
        });
    }

    let inspect = Command::new("podman")
        .args([
            "image",
            "inspect",
            "--format",
            "{{.Digest}}",
            artifact.reference(),
        ])
        .output()
        .map_err(|source| PullImageError::Execute {
            operation: "inspecting an image",
            source,
        })?;
    let inspect_stdout = String::from_utf8_lossy(&inspect.stdout).into_owned();
    let inspect_stderr = String::from_utf8_lossy(&inspect.stderr).into_owned();
    if !inspect.status.success() {
        return Err(PullImageError::Inspect {
            reference: artifact.reference().to_owned(),
            stdout: inspect_stdout,
            stderr: inspect_stderr,
        });
    }

    let Some(actual) = normalize_digest(&inspect_stdout) else {
        return Err(PullImageError::InvalidInspectOutput {
            reference: artifact.reference().to_owned(),
            output: inspect_stdout,
        });
    };
    if actual != artifact.digest() {
        return Err(PullImageError::DigestMismatch {
            reference: artifact.reference().to_owned(),
            expected: artifact.digest().to_owned(),
            actual: actual.to_owned(),
        });
    }

    Ok(PulledImage { artifact })
}

#[derive(Debug)]
pub enum ResolveImageDigestError {
    Execute {
        operation: &'static str,
        source: io::Error,
    },
    Pull {
        reference: String,
        stdout: String,
        stderr: String,
    },
    Inspect {
        reference: String,
        stdout: String,
        stderr: String,
    },
    InvalidInspectOutput {
        reference: String,
        output: String,
    },
}

impl fmt::Display for ResolveImageDigestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Execute { operation, source } => write!(
                formatter,
                "failed to execute Podman while {operation}: {source}"
            ),
            Self::Pull {
                reference,
                stdout,
                stderr,
            } => write!(
                formatter,
                "failed to pull image `{reference}` with Podman: {}",
                diagnostic(stdout, stderr)
            ),
            Self::Inspect {
                reference,
                stdout,
                stderr,
            } => write!(
                formatter,
                "failed to inspect image `{reference}` with Podman: {}",
                diagnostic(stdout, stderr)
            ),
            Self::InvalidInspectOutput { reference, output } => write!(
                formatter,
                "Podman returned an invalid digest while inspecting image `{reference}`: {output}"
            ),
        }
    }
}

impl Error for ResolveImageDigestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source, .. } => Some(source),
            Self::Pull { .. } | Self::Inspect { .. } | Self::InvalidInspectOutput { .. } => None,
        }
    }
}

// Resolves a CI commit tag to a validated immutable artifact for Release creation.
pub fn resolve_image_digest(
    repository: &OciRepository,
    commit_sha: &CommitSha,
) -> Result<OciArtifact, ResolveImageDigestError> {
    let tagged = format!("{}:{}", repository.as_str(), commit_sha.as_str());

    let pull = Command::new("podman")
        .args(["pull", "--quiet", &tagged])
        .output()
        .map_err(|source| ResolveImageDigestError::Execute {
            operation: "pulling an image by tag",
            source,
        })?;
    let pull_stdout = String::from_utf8_lossy(&pull.stdout).into_owned();
    let pull_stderr = String::from_utf8_lossy(&pull.stderr).into_owned();
    if !pull.status.success() {
        return Err(ResolveImageDigestError::Pull {
            reference: tagged.clone(),
            stdout: pull_stdout,
            stderr: pull_stderr,
        });
    }

    let inspect = Command::new("podman")
        .args(["image", "inspect", "--format", "{{.Digest}}", &tagged])
        .output()
        .map_err(|source| ResolveImageDigestError::Execute {
            operation: "inspecting an image",
            source,
        })?;
    let inspect_stdout = String::from_utf8_lossy(&inspect.stdout).into_owned();
    let inspect_stderr = String::from_utf8_lossy(&inspect.stderr).into_owned();
    if !inspect.status.success() {
        return Err(ResolveImageDigestError::Inspect {
            reference: tagged,
            stdout: inspect_stdout,
            stderr: inspect_stderr,
        });
    }

    let Some(digest) = normalize_digest(&inspect_stdout) else {
        return Err(ResolveImageDigestError::InvalidInspectOutput {
            reference: tagged,
            output: inspect_stdout,
        });
    };

    OciArtifact::new(repository.as_str(), digest).map_err(|_| {
        ResolveImageDigestError::InvalidInspectOutput {
            reference: tagged,
            output: digest.to_owned(),
        }
    })
}

// Accepts only canonical lowercase SHA-256 digests returned by Podman inspection.
fn is_sha256_digest(digest: &str) -> bool {
    digest
        .strip_prefix(DIGEST_ALGORITHM)
        .is_some_and(|hex| hex.len() == SHA256_HEX_LENGTH && hex.bytes().all(is_lowercase_hex))
}

// Validates one digest byte without accepting uppercase normalization variants.
fn is_lowercase_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

// Trims command output and returns a digest only when it is safe to persist as artifact identity.
fn normalize_digest(output: &str) -> Option<&str> {
    let digest = output.trim();
    is_sha256_digest(digest).then_some(digest)
}

// Prefers Podman's stderr failure detail, using stdout only when stderr is empty.
fn diagnostic<'a>(stdout: &'a str, stderr: &'a str) -> &'a str {
    if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_digest_pinned_image_reference() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let reference =
            OciArtifact::parse(&format!("registry.example/app/image@{digest}")).unwrap();

        assert_eq!(reference.repository(), "registry.example/app/image");
        assert_eq!(reference.digest(), digest);
    }

    #[test]
    fn rejects_references_without_a_lowercase_sha256_digest() {
        let invalid_references = [
            "registry.example/app",
            "@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "registry.example/app@sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "registry.example/app@sha512:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ];

        for reference in invalid_references {
            assert!(OciArtifact::parse(reference).is_err());
        }
    }

    #[test]
    fn normalizes_podman_digest_output() {
        let digest = format!("sha256:{}", "b".repeat(64));

        assert_eq!(
            normalize_digest(&format!("\n{digest}\n")),
            Some(digest.as_str())
        );
        assert_eq!(normalize_digest("sha256:not-a-digest"), None);
    }
}
