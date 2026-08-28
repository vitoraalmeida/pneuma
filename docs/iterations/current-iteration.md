# Current Iteration

**Status:** em andamento

**Base:** `3671d53` (`docs: synchronize living documents with the v0.4.3 release`)

**Approved design:**
[`../designs/greenfield-architecture-simplification.md`](../designs/greenfield-architecture-simplification.md)
(approved 2026-08-28)

## Iteration — Greenfield Architecture Simplification

Objective: replace obsolete compatibility, ownership, and migration mechanisms
with the approved smaller current architecture. The database reset is explicitly
incompatible; no existing database or backup is upgraded.

## Checkpoints

1. [x] Current ADR set and per-Application coordination
   - Replace the ADR set with the six current decisions.
   - Use one per-Application kernel lock for every existing-Application mutation
     and remove operation tokens, generations, ownership storage, and related
     terminology.
   - Preserve targeted CAS only where an exact persisted precondition matters.
   - Result: deploy, rollback, lifecycle, status observation, visibility, and
     reconciliation hold the same lock; the operation store and migration are
     removed, and contention is caller-visible (`Deferred` for reconciliation).
2. [x] Strict current domain types
   - Remove compatibility-only System, manifest-version, source-revision,
     failure-evidence, delivery, and local-source representations.
   - Validate current entity IDs at generation, boundaries, and hydration.
   - Result: `Application.system_id` is a required `SystemId`; manifest schema
     version is an import-boundary check only; source revisions are optional
     validated `CommitSha`s; failed deployments carry complete typed evidence
     (`DeploymentFailureCode` end to end); entity IDs validate the 32-character
     lowercase-hex format at generation and hydration; `DeliveryType`,
     `RepositoryKind`, local sources, and all legacy tolerances are removed,
     with obsolete rows failing hydration explicitly.
3. [ ] Exact baseline schema and stores
   - Replace the historical chain with the one exact eight-table baseline,
     current ledger identity, constraints, and current SQL mappings.
4. [ ] Boundary and workflow proof
   - Prove checkout, Caddy, and runtime identity at external boundaries and make
     lifecycle success depend on observed target state.
5. [ ] Database replacement safety
   - Serialize normal database access and restore with a database-wide kernel
     lock and accept only exact-current backups.
6. [ ] Living documentation and invariant consolidation
   - Synchronize implemented documentation and replace the invariant inventory
     with the approved compact durable guarantees.
7. [ ] Operational regression and closure
   - Run final CI, applicable rootless-Podman and disposable-host evidence, then
     close only with every acceptance criterion proved.

## Acceptance Criteria

- Every checkpoint meets the acceptance scenarios in the approved design without
  adding its stated non-goals.
- Each code checkpoint has focused tests plus `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`.
- Schema, CLI, shell, documentation, and environment-dependent checks have the
  evidence required by their respective checkpoint; unavailable environments are
  recorded as skips, never passes.

## Blockers

- None.

## Validation Evidence

- Checkpoint 0: the approved design is indexed, this is the sole active tracker,
  and Checkpoint 1 is the unambiguous next implementation checkpoint.
- Checkpoint 1: `cargo fmt --check`, Clippy with warnings denied, all-feature
  tests, release build, markdown-link validation, and `bash -n` passed. The
  three ignored OCI tests require a configured rootless Podman host. ShellCheck
  was unavailable on this host and was not reported as passed.
- Checkpoint 2: `cargo fmt --check`, Clippy with warnings denied, all-feature
  tests (25 suites green; the same three ignored OCI tests remain
  environment-dependent), and release build passed.
