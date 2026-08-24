# ADR-0006 - Release, Deployment, and RuntimeInstance Model

**Status:** Accepted (retrospective)

## Context

Artifact identity, activation attempts, and concrete runtime materialization have
different lifecycles. Combining them loses history and makes rollback ambiguous.

## Decision

Pneuma models them separately:

```text
Release R17: immutable sha256:abcd artifact
Deployment D40: deploy R17, failed
Deployment D41: deploy R17, succeeded
RuntimeInstance RI30: materialized by D41
Deployment D42: rollback to an earlier Release
```

A Release is reusable and identified by `(application, digest)`. A Deployment is
one activation attempt. A RuntimeInstance is the logical materialization created
by that attempt. Rollback creates a new Deployment; it does not reactivate or
rewrite a historical one.

Activation is health-gated: a Deployment runs its candidate as a private
loopback runtime and may only become the active Deployment after the persisted
health check passes (internal check always; public applications additionally
require Caddy materialization plus external health through the route). A failed
candidate is never promoted — the previously active runtime and public route
remain in use — and the failure is recorded as terminal evidence on the
candidate's own Deployment row.

## Alternatives Considered

- **One mutable current deployment record:** rejected because it erases failed
  attempts and rollback history.
- **Container ID as runtime identity:** rejected because Quadlet can recreate a
  container while the logical runtime remains the same.
- **Promote first, verify later:** rejected because a broken replacement would
  already own the active slot and route; health gating keeps the working version
  serving until the candidate is proven.

## Consequences

The model preserves activation history, supports digest reuse, and separates
logical runtime identity from an external container ID. Health gating means
every deployment pays the verification latency before activation, and public
deployments additionally depend on Caddy reload plus external health succeeding
before promotion.

## References

- [`../architecture/data-model.md`](../architecture/data-model.md)
- [`../architecture/architecture.md`](../architecture/architecture.md)
