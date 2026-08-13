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

## Alternatives Considered

- **One mutable current deployment record:** rejected because it erases failed
  attempts and rollback history.
- **Container ID as runtime identity:** rejected because Quadlet can recreate a
  container while the logical runtime remains the same.

## Consequences

The model preserves activation history, supports digest reuse, and separates
logical runtime identity from an external container ID.

## References

- [`../architecture/data-model.md`](../architecture/data-model.md)
- [`../architecture/architecture.md`](../architecture/architecture.md)
