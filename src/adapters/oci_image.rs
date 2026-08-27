use std::io;
use std::process::Command;

use thiserror::Error;

use crate::domain::git::CommitSha;
use crate::domain::release::{OciArtifact, OciRepository, is_sha256_digest};

// Pulls a digest-pinned artifact and confirms Podman resolved exactly that digest.
#[derive(Debug, Error)]
pub enum PullImageError {
    #[error("failed to execute Podman while {operation}: {source}")]
    Execute {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error(
        "failed to pull image `{reference}` with Podman: {}",
        diagnostic(stdout, stderr)
    )]
    Pull {
        reference: String,
        stdout: String,
        stderr: String,
    },
    #[error(
        "failed to inspect image `{reference}` with Podman: {}",
        diagnostic(stdout, stderr)
    )]
    Inspect {
        reference: String,
        stdout: String,
        stderr: String,
    },
    #[error("Podman returned an invalid digest while inspecting image `{reference}`: {output}")]
    InvalidInspectOutput { reference: String, output: String },
    #[error("Podman inspected image `{reference}` as `{actual}`, expected `{expected}`")]
    DigestMismatch {
        reference: String,
        expected: String,
        actual: String,
    },
}

// Pulls a digest-pinned artifact and confirms Podman resolved exactly that digest.
pub(crate) fn pull_image(artifact: &OciArtifact) -> Result<(), PullImageError> {
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

    Ok(())
}

#[derive(Debug, Error)]
pub enum ResolveImageDigestError {
    #[error("failed to execute Podman while {operation}: {source}")]
    Execute {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error(
        "failed to pull image `{reference}` with Podman: {}",
        diagnostic(stdout, stderr)
    )]
    Pull {
        reference: String,
        stdout: String,
        stderr: String,
    },
    #[error(
        "failed to inspect image `{reference}` with Podman: {}",
        diagnostic(stdout, stderr)
    )]
    Inspect {
        reference: String,
        stdout: String,
        stderr: String,
    },
    #[error("Podman returned an invalid digest while inspecting image `{reference}`: {output}")]
    InvalidInspectOutput { reference: String, output: String },
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
            reference: tagged,
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
    use std::path::PathBuf;

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

    // Fake `podman` for pull/resolution contract tests. Every invocation is
    // logged; inspect answers with PNEUMA_FAKE_PODMAN_DIGEST.
    const FAKE_PODMAN: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_PODMAN_LOG\"
if [ \"$1 $2\" = \"image inspect\" ]; then
  printf '%s\\n' \"$PNEUMA_FAKE_PODMAN_DIGEST\"
  exit \"${PNEUMA_FAKE_PODMAN_INSPECT_EXIT:-0}\"
fi
exit \"${PNEUMA_FAKE_PODMAN_PULL_EXIT:-0}\"
";

    struct ScopedPodman {
        _path: crate::test_support::ScopedExternalPath,
        log: PathBuf,
    }

    impl ScopedPodman {
        const BEHAVIOR_VARIABLES: [&str; 3] = [
            "PNEUMA_FAKE_PODMAN_DIGEST",
            "PNEUMA_FAKE_PODMAN_INSPECT_EXIT",
            "PNEUMA_FAKE_PODMAN_PULL_EXIT",
        ];

        fn new(name: &str, digest: &str) -> Self {
            let path =
                crate::test_support::ScopedExternalPath::new(name, &[("podman", FAKE_PODMAN)]);
            for variable in Self::BEHAVIOR_VARIABLES {
                path.remove_var(variable);
            }
            let log = path.directory().join("invocations.log");
            path.set_var("PNEUMA_FAKE_PODMAN_LOG", &log.to_string_lossy());
            path.set_var("PNEUMA_FAKE_PODMAN_DIGEST", digest);
            Self { _path: path, log }
        }

        fn invocations(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    fn artifact(digest_character: char) -> OciArtifact {
        OciArtifact::parse(&format!(
            "registry.example/app@sha256:{}",
            digest_character.to_string().repeat(64)
        ))
        .unwrap()
    }

    #[test]
    fn pull_image_pulls_the_pinned_reference_and_confirms_the_digest() {
        let scoped = ScopedPodman::new("pull-verified", &format!("sha256:{}", "a".repeat(64)));

        pull_image(&artifact('a')).unwrap();

        assert_eq!(
            scoped.invocations(),
            [
                format!("pull {}", artifact('a').reference()),
                format!(
                    "image inspect --format {{{{.Digest}}}} {}",
                    artifact('a').reference()
                ),
            ]
        );
    }

    #[test]
    fn pull_image_refuses_a_digest_mismatch_invalid_output_and_failed_pulls() {
        // A registry serving different bytes than the declared artifact is a hard error.
        {
            let _scoped = ScopedPodman::new("pull-mismatch", &format!("sha256:{}", "b".repeat(64)));
            let error = pull_image(&artifact('a')).unwrap_err();
            assert!(matches!(
                error,
                PullImageError::DigestMismatch { expected, actual, .. }
                    if expected == format!("sha256:{}", "a".repeat(64))
                        && actual == format!("sha256:{}", "b".repeat(64))
            ));
        }

        {
            let _scoped = ScopedPodman::new("pull-invalid", "\nsha256:not-a-digest\n");
            assert!(matches!(
                pull_image(&artifact('a')),
                Err(PullImageError::InvalidInspectOutput { .. })
            ));
        }

        {
            let scoped = ScopedPodman::new("pull-failure", "");
            scoped._path.set_var("PNEUMA_FAKE_PODMAN_PULL_EXIT", "5");
            assert!(matches!(
                pull_image(&artifact('a')),
                Err(PullImageError::Pull { .. })
            ));
        }
    }

    #[test]
    fn resolve_image_digest_builds_the_tagged_reference_and_normalizes_the_answer() {
        let repository = OciRepository::new("registry.example/app").unwrap();
        let commit = CommitSha::new(&"c".repeat(40)).unwrap();
        let digest = format!("sha256:{}", "d".repeat(64));
        let scoped = ScopedPodman::new("resolve-digest", &format!("\n{digest}\n"));

        let resolved = resolve_image_digest(&repository, &commit).unwrap();

        assert_eq!(resolved.repository(), repository.as_str());
        assert_eq!(resolved.digest(), digest);
        assert_eq!(
            scoped.invocations(),
            [
                format!("pull --quiet registry.example/app:{}", commit.as_str()),
                format!(
                    "image inspect --format {{{{.Digest}}}} registry.example/app:{}",
                    commit.as_str()
                ),
            ]
        );
        drop(scoped);

        {
            let failing = ScopedPodman::new("resolve-digest-failure", "");
            failing._path.set_var("PNEUMA_FAKE_PODMAN_PULL_EXIT", "7");
            assert!(matches!(
                resolve_image_digest(&repository, &commit),
                Err(ResolveImageDigestError::Pull { .. })
            ));
        }
    }
}
