use std::error::Error;
use std::fmt;
use std::io;
use std::process::Command;

const DIGEST_ALGORITHM: &str = "sha256:";
const SHA256_HEX_LENGTH: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub struct OciImageReference {
    value: String,
    repository: String,
    digest: String,
}

impl OciImageReference {
    pub fn parse(value: &str) -> Result<Self, InvalidImageReference> {
        let Some((repository, digest)) = value.split_once('@') else {
            return Err(InvalidImageReference {
                reference: value.to_owned(),
            });
        };
        if !is_repository(repository) || !is_sha256_digest(digest) {
            return Err(InvalidImageReference {
                reference: value.to_owned(),
            });
        }

        Ok(Self {
            value: value.to_owned(),
            repository: repository.to_owned(),
            digest: digest.to_owned(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidImageReference {
    pub reference: String,
}

impl fmt::Display for InvalidImageReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "image reference `{}` must be <repository>@sha256:<64 lowercase hexadecimal characters>",
            self.reference
        )
    }
}

impl Error for InvalidImageReference {}

#[derive(Debug, PartialEq, Eq)]
pub struct PulledImage {
    pub reference: OciImageReference,
}

#[derive(Debug)]
pub enum PullImageError {
    InvalidReference {
        source: InvalidImageReference,
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

pub fn pull_image(reference: &str) -> Result<PulledImage, PullImageError> {
    let reference = OciImageReference::parse(reference)
        .map_err(|source| PullImageError::InvalidReference { source })?;
    let pull = Command::new("podman")
        .args(["pull", reference.as_str()])
        .output()
        .map_err(|source| PullImageError::Execute {
            operation: "pulling an image",
            source,
        })?;
    let pull_stdout = String::from_utf8_lossy(&pull.stdout).into_owned();
    let pull_stderr = String::from_utf8_lossy(&pull.stderr).into_owned();
    if !pull.status.success() {
        return Err(PullImageError::Pull {
            reference: reference.as_str().to_owned(),
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
            reference.as_str(),
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
            reference: reference.as_str().to_owned(),
            stdout: inspect_stdout,
            stderr: inspect_stderr,
        });
    }

    let Some(actual) = normalize_digest(&inspect_stdout) else {
        return Err(PullImageError::InvalidInspectOutput {
            reference: reference.as_str().to_owned(),
            output: inspect_stdout,
        });
    };
    if actual != reference.digest() {
        return Err(PullImageError::DigestMismatch {
            reference: reference.as_str().to_owned(),
            expected: reference.digest().to_owned(),
            actual: actual.to_owned(),
        });
    }

    Ok(PulledImage { reference })
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

fn normalize_digest(output: &str) -> Option<&str> {
    let digest = output.trim();
    is_sha256_digest(digest).then_some(digest)
}

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
            OciImageReference::parse(&format!("registry.example/app/image@{digest}")).unwrap();

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
            assert!(OciImageReference::parse(reference).is_err());
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
