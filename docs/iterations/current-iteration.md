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
3. [x] Exact baseline schema and stores
   - Replace the historical chain with the one exact eight-table baseline,
     current ledger identity, constraints, and current SQL mappings.
   - Result: `migrations/0001_current_schema.sql` replaces migrations 0001-0014
     with the eight-table baseline (flattened Application specification,
     canonical Release `image_reference`, no runtime `host_address` or `removed`
     state, composite ownership foreign keys, evidence CHECKs, one-in-progress
     Deployment index, case-insensitive public-domain uniqueness, and
     one-reservation-per-Deployment). The runner initializes empty databases
     atomically, reopens the exact current textual ledger, and rejects every
     other schema as incompatible; reservation allocation is idempotent per
     Deployment and registration consumes the exact reservation.
4. [x] Boundary and workflow proof
    - Prove checkout, Caddy, and runtime identity at external boundaries and make
      lifecycle success depend on observed target state.
    - Result: the checkout boundary is exactly the production Git effects
      (`clone_repository`, `resolve_branch`, `cleanup_checkout`) and the retired
      local-checkout machinery (`resolve_commit`, `create_checkout`,
      `ensure_checkout`) is removed with its tests. Container destruction is now
      proven by observation: retirement and failed-candidate cleanup observe
      absence first (Quadlet's ExecStop removes the supervised container), force-
      remove only a still-present container, re-observe, and record retirement or
      missing state only after observed absence, with unproven removals reported
      as warnings or `ContainerNotRemoved` divergence instead of silent success.
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
- Checkpoint 3: `cargo fmt --check`, Clippy with warnings denied, all-feature
  tests (25 suites green; the same three ignored OCI tests remain
  environment-dependent), and release build passed. Fresh initialization,
  idempotent reopen, incompatible-schema rejection (including the retired
  integer ledger), baseline transaction rollback, and representative constraint
  tests are green in `src/adapters/database.rs`.
- Checkpoint 4: `cargo fmt --check`, Clippy with warnings denied, all-feature
  tests (25 suites green; the same three ignored OCI tests remain
  environment-dependent), and release build passed. New proofs: adapter tests
  for `container_exists`, retirement recorded only after observed container
  absence (including the Quadlet-ExecStop path without force removal), and
  candidate cleanup divergence when removal cannot be proven; real-git checkout
  tests cover the surviving boundary only.
