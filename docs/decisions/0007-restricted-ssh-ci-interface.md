# ADR-0007 - Restricted SSH CI Interface

**Status:** Accepted (retrospective)

## Context

CI needs to request deployments on the host but does not need an interactive
shell or arbitrary command execution.

## Decision

Bootstrap installs the CI public key with `restrict` and a forced
`pneuma ci dispatch` command. The dispatcher reads `SSH_ORIGINAL_COMMAND` and
accepts only `version` or `deploy <application> <branch-or-tag>`, validating the
argument forms before it invokes the normal branch-deployment flow.

## Alternatives Considered

- **Normal SSH shell:** rejected because CI deployment capability should not
  imply arbitrary host command execution.
- **CI directly invoking Podman/Caddy:** rejected because it bypasses Pneuma's
  deployment validation and persistence rules.

## Consequences

Compromise of this key is constrained to dispatcher operations, but it can still
request deployment for any imported Application and syntactically valid branch or
tag. There is no per-key Application or branch authorization.

## References

- [`../architecture/security-model.md`](../architecture/security-model.md)
- [`../getting-started.md`](../getting-started.md)
- [`../architecture/architecture.md`](../architecture/architecture.md)
