# ADR-0005 — Application operation coordination

**Status:** Accepted

## Context

Concurrent CLI invocations can otherwise interleave state-dependent reads,
managed runtime effects, confirmations, and compensation for one Application.

## Decision

One kernel-backed lock per Application is the sole live operation-ownership
mechanism. Every existing-Application mutation holds it from its first
state-dependent read through effects, confirmation, and compensation. Contention
is an explicit busy/conflict result; reconciliation returns `Deferred`.

Compare-and-set is used only when correctness depends on an exact persisted
precondition. A zero-row conditional write is conflict or not-converged, while
ordinary locked telemetry, structural constraints, and idempotent deletes do not
gain artificial conditional fields. No SQLite transaction spans an external
effect.

## Consequences

Unrelated Applications remain independent and process death releases a held
lock. Operation tokens, persisted generations, and ownership records are not
part of the coordination model.
