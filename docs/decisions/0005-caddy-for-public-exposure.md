# ADR-0005 - Caddy for Public Exposure Materialization

**Status:** Accepted (retrospective)

## Context

An Application's runtime lifecycle and public reachability are distinct. A
running runtime can remain internal, and removing public exposure must not stop
the application.

## Decision

Pneuma binds runtimes to loopback and materializes public routes as managed Caddy
fragments. SQLite stores desired visibility and confirmed route materialization;
Caddy remains authoritative for the fragment, reload, and route behavior.

## Alternatives Considered

- **Direct container port exposure:** rejected because it weakens the explicit
  ingress boundary and couples runtime state to public reachability.
- **nginx or Traefik:** viable alternatives, but Caddy is the implemented host
  ingress integration and provides the required managed-fragment workflow.

## Consequences

Changing visibility to internal removes only the managed route and leaves the
loopback runtime available. Public promotion requires Caddy validation, reload,
and external health confirmation.

## References

- [`../architecture/architecture.md`](../architecture/architecture.md)
- [`../architecture/data-model.md`](../architecture/data-model.md)
- [`../architecture/security-model.md`](../architecture/security-model.md)
