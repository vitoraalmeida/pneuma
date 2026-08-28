# ADR-0004 — State authority and reconciliation

**Status:** Accepted

## Context

Persisted intent cannot prove the current state of Podman, systemd, or Caddy.
Conversely, external observation cannot replace logical identity or history.

## Decision

SQLite owns intent, logical identity, history, and confirmed results. External
systems own current observations. Reconciliation is a pure, conservative
decision over typed desired, persisted, observed, and expected facts; its decision
performs no effects and never invents a Release, Deployment, or certainty.

## Consequences

Unknown and missing observations stay explicit. Reconciliation repairs only
unambiguous identity-preserving drift and otherwise requires manual intervention.
