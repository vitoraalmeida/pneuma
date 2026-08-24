# ADR-0008 - Reconciliation As Pure Decision Separated From Effects

**Status:** Accepted (retrospective)

## Context

Pneuma records intent and bookkeeping in SQLite while Podman/systemd/Caddy hold
the real runtime and route state. After crashes, manual edits, reboots, or
interrupted deployments these can diverge. Deciding what to do about drift inside
the same code that performs effects makes that logic untestable without live
infrastructure and invites repairs based on guessed facts.

## Decision

Reconciliation is a pipeline with a pure core:

```text
load persisted facts -> observe external facts -> render canonical expectations
-> decide(input, observation, expectations) -> execute decided variant -> confirm via CAS
```

The decision function lives in the domain (`domain/reconciliation.rs::decide`)
and has no access to SQLite, Podman, systemd, Caddy, the filesystem, clocks, or
randomness. It returns one explicit action per application state: in-sync,
runtime identity repair, rematerialization, internal-route removal,
public-route materialization, public-exposure failure record, deferred, or
manual intervention with a reason. Use cases only acquire ownership, order the
steps, and execute; adapters own observation and canonical bytes.

Conservatism rules are part of the decision contract:

- a `Missing` container is an observation, never a license to invent resources;
  rematerialization requires unambiguous persisted identity;
- diverged exposure is never auto-repaired; it demands manual intervention;
- unknown external states stay explicitly unknown instead of being adopted as
  known-safe stopped/running values;
- drift that no safe rule covers is reported (`UnhandledDrift`/manual
  intervention) rather than silently ignored.

## Alternatives Considered

- **Decide-and-fix inline in the use case:** rejected because the full drift
  matrix could then only be tested against live Podman/systemd/Caddy, and effect
  code would grow implicit policy.
- **Aggressive auto-repair (recreate whatever is missing):** rejected because a
  recreated container with a foreign image, wrong labels, or divergent endpoint
  would be silently adopted as the application's runtime.
- **Alert-only, no repair:** rejected because reboot recovery and interrupted
  deployments need automatic convergence for provably safe cases.

## Consequences

The entire drift decision matrix is unit-testable without infrastructure, new
drift cases fail loudly instead of guessing, and repair effects are individually
idempotent and CAS-confirmed. The cost is that ambiguous situations surface as
manual intervention rather than being resolved automatically, and every new
observed fact must be threaded through the typed input model.

## References

- [`../architecture/invariants.md`](../architecture/invariants.md) (INV-REC-001..INV-REC-005)
- [`../architecture/architecture.md`](../architecture/architecture.md) (Ownership - Reconciliation)
