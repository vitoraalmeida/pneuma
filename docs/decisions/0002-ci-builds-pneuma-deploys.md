# ADR-0002 - CI Builds, Pneuma Deploys

**Status:** Accepted (retrospective)

## Context

Pneuma needs a reproducible artifact identity for deployment. Building
applications on the deployment host couples application toolchains, build
credentials, and host operation, and obscures the artifact that was activated.

## Decision

CI builds and publishes OCI images. Pneuma imports application configuration from
Git, resolves a requested branch or tag to a full commit SHA, pulls
`repository:<commit-sha>`, resolves its digest, and deploys the resulting
`repository@sha256:...` reference.

## Alternatives Considered

- **Build on the VPS:** rejected because it expands the host's toolchain and
  attack surface and couples build and runtime concerns.
- **Manual digest input only:** remains supported for explicit deployment but is
  insufficient as the normal Git/CI delivery flow.
- **Use mutable `latest`:** rejected because it can silently select a different
  artifact.

## Consequences

Pneuma can deploy immutable digests and refuses unavailable commit artifacts
without falling back to a prior image, local build, or `latest`. It assumes CI
publishes the expected commit-SHA tag; it does not prove artifact publisher
provenance or verify image signatures.

## References

- [`../architecture/architecture.md`](../architecture/architecture.md)
- [`../architecture/security-model.md`](../architecture/security-model.md)
- [`../CHANGELOG.md`](../../CHANGELOG.md)
