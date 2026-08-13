# ADR-0001 - Single-Host Platform

**Status:** Accepted (retrospective)

## Context

Pneuma needs production-like deployment properties for a small set of
applications: immutable artifacts, health validation, rollback history, durable
intent, and controlled public exposure. It does not currently need scheduling or
reconciliation across a fleet.

## Decision

Pneuma intentionally operates one Linux host. SQLite, rootless Podman, systemd
Quadlet, and Caddy run locally on that host.

## Alternatives Considered

- **Kubernetes:** provides scheduling, reconciliation, and multi-node
  capabilities, but introduces cluster and control-plane operation outside the
  current scale and scope.
- **Compose alone:** provides a simpler familiar runtime definition, but does
  not provide Pneuma's first-class Release, Deployment, candidate validation,
  promotion, and persisted-intent semantics without separate orchestration.
- **Custom resident orchestrator:** would add a continuously available controller
  without a demonstrated need.

## Consequences

Pneuma avoids distributed control-plane complexity and remains locally
inspectable. It cannot schedule, tolerate host failure, or coordinate workloads
across hosts.

## References

- [`../architecture/system-context.md`](../architecture/system-context.md)
- [`../architecture/architecture.md`](../architecture/architecture.md)
- [`../roadmap.md`](../roadmap.md)
