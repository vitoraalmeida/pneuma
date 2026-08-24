# ADR-0009 - Kernel Lock, Ownership Generations, And Compare-And-Set

**Status:** Accepted (retrospective)

## Context

Multiple Pneuma invocations can run concurrently (two deploys, deploy plus
reconcile), and a process can die mid-operation leaving an old owner's token in
the database. SQLite serializes individual statements but cannot by itself
express "this write is only valid if the world still looks like when I decided".

## Decision

Three coordinated mechanisms protect every state-racing mutation:

1. **Per-application kernel lock** (`flock`): deploy and reconcile acquire it
   before long work; reconcile defers while it is held. The lock file is never
   unlinked, so the inode is stable and process death releases the lock.
2. **Monotonic ownership generations**: taking ownership atomically advances a
   per-application generation and replaces the token (`operation_store`). A
   displaced owner holds a superseded epoch; its later guarded writes lose.
3. **Compare-and-set writes**: every UPDATE that races on state carries the
   expected prior value. Zero rows updated means stale/concurrent state — an
   explicit typed conflict for the caller, never silent success.

Use cases persist intent before external effects and confirm after observing
them, so a lost CAS after an effect triggers compensation instead of a false
success report.

## Alternatives Considered

- **Global process-wide mutex:** rejected because it does not survive multiple
  processes and turns unrelated applications into contention.
- **Rely on workflow checks alone (read then write):** rejected because two
  processes can pass the same check; only conditional writes make
  check-and-write indivisible.
- **A resident daemon serializing operations:** rejected because Pneuma is a
  short-lived CLI; coordination must live in the filesystem and database.

## Consequences

Concurrent mutations are safe without a coordinator process: one wins, losers
get typed conflicts, and interrupted epochs cannot overwrite newer state. The
costs are that every racing store primitive must carry its CAS predicate, and
callers must translate `Stale` outcomes into compensation or retry instead of
assuming success.

## References

- [`../architecture/invariants.md`](../architecture/invariants.md) (INV-WF-007, INV-DB-004, INV-DB-005)
- [`../architecture/architecture.md`](../architecture/architecture.md) (Authority and Persistence)
