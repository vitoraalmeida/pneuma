# ADR-0004 - SQLite Intent and External Runtime Authority

**Status:** Accepted (retrospective)

## Context

Pneuma needs durable logical identity and deployment history, but a database row
cannot prove that a container, systemd unit, or Caddy route currently exists.

## Decision

SQLite owns desired intent, logical identities, history, and last confirmed
results. Podman/systemd own observed runtime state; Caddy owns materialized route
state; Git owns branch resolution; the OCI registry owns artifact availability
and digest information.

## Alternatives Considered

- **Treat SQLite as the live runtime authority:** rejected because external
  resources can disappear or change independently.
- **Persist no logical state:** rejected because rollback, durable intent, and
  deployment history need stable local identities.

## Consequences

Use cases persist intent before an external effect and confirmed results after
observation, with short transactions and compare-and-set writes. Future
reconciliation compares persisted intent with external observation rather than
assuming they are identical.

## References

- [`../architecture/architecture.md`](../architecture/architecture.md)
- [`../architecture/data-model.md`](../architecture/data-model.md)
