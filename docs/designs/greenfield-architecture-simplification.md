# Greenfield Architecture Simplification

**Status:** approved on 2026-08-28

## Purpose

Replace Pneuma's accumulated compatibility and coordination mechanisms with a
small current architecture. This is an intentionally incompatible database
reset for a repository with no external users; Git history preserves the
retired design and migration chain.

## Fixed Decisions

1. A kernel-backed lock scoped to one Application is the sole live-operation
   ownership mechanism. Every mutation of that Application's persisted or
   managed Podman, systemd, or Caddy state holds it from the first
   state-dependent read through confirmation and compensation. Contention is a
   busy/conflict result, or `Deferred` for reconciliation. Imports rely on
   SQLite uniqueness until an Application exists.
2. Compare-and-set writes apply only when a decision needs an exact prior
   mutable state. A zero-row conditional write is conflict or not-converged;
   ordinary writes, lock-protected telemetry, structural constraints, and
   idempotent deletes do not acquire artificial CAS fields.
3. Operation tokens, ownership generations, operation tables, global row
   versions, and compatibility states are removed and not replaced.
4. Domain and persistence accept current values only. Systems are required on
   Applications; manifest version is an import-boundary check; source revision
   is an optional current commit; failures carry complete typed evidence;
   identifiers use the current lowercase hexadecimal format; OCI delivery and
   remote Git source each have one supported representation.
5. Releases contain one canonical digest-pinned OCI artifact. Runtime unknown
   status text remains explicit; retirement uses lifecycle state plus
   `removed_at`, never a persisted `removed` pseudo-state.
6. The executable migration history is replaced by one current baseline and a
   small textual-identity migration ledger. Empty databases initialize
   atomically; only the exact current schema reopens; every other non-empty
   schema, backup, or restore source fails explicitly as incompatible.
7. The current schema has exactly these tables: `schema_migrations`, `systems`,
   `applications`, `releases`, `deployments`, `runtime_instances`, `exposures`,
   and `runtime_port_reservations`. SQLite constraints and immediate
   transactions enforce ownership, lifecycle evidence, one in-progress
   deployment, route/domain identity, and port-reservation exclusivity.
8. Workflows remain local sagas: persist intent/reservation before an external
   effect, keep no SQLite transaction across effects, observe, then confirm.
   Reconciliation is a pure conservative decision over typed facts and never
   invents a Release, Deployment, or certainty.
9. Database access uses a database-wide kernel lock: shared for normal commands
   and exclusive for restore. CI remains a forced restricted command that
   validates arguments and dispatches through normal use cases.

## Incompatibility and Non-Goals

- Existing databases, historical migration ledgers, legacy rows, and backups
  are not upgraded, backfilled, or imported.
- No daemon, async control plane, distributed/multi-host coordination, generic
  repositories, service containers, plugins, factories, or transaction
  frameworks are introduced.
- No persisted fencing/idempotency framework, automatic cleanup retry state,
  signing/SBOM work, resource policy, network policy, audit identity, or queued
  v0.5 product scope is included.

## Acceptance Scenarios

- Same-Application mutations exclude one another, lock contention is explicit,
  lock release follows process death, and unrelated Applications proceed
  independently.
- Malformed or obsolete boundary values and persisted rows fail explicitly;
  current values hydrate into validated domain types only.
- A fresh database initializes and reopens; an old or malformed schema is
  rejected; constraints prevent invalid ownership, evidence, active identity,
  domain, and port states.
- Candidate failure preserves the prior active runtime and route; only proven
  external identity is adopted or destroyed; lifecycle success follows observed
  target state.
- Restore cannot race normal access and rejects an incompatible backup.
- Documentation contains only the six current ADRs and the compact durable
  invariant inventory; all required Rust, documentation, shell, and applicable
  disposable-host checks pass.

## Checkpoint Order

1. Current ADR set and per-Application coordination.
2. Strict current domain types.
3. Exact baseline schema and stores.
4. Boundary and workflow proof.
5. Database replacement safety.
6. Living documentation and invariant consolidation.
7. Operational regression and iteration closure.

Each checkpoint is independently green, updates the active tracker and only
implemented documentation, and does not begin the next checkpoint early.
