# Current Iteration

**Status:** em andamento

**Base:** `5b0f564` (`docs: approve CLI adapter consolidation design`)

**Approved design:**
[`../designs/cli-adapter-consolidation.md`](../designs/cli-adapter-consolidation.md)
(approved 2026-09-01)

## Iteration - CLI Adapter Consolidation

Objective: consolidate the CLI adapter so parsed interactive arguments map
directly to the existing control command vocabulary and one adapter flow owns
execution and result rendering. This is a structural refactor: CLI syntax,
text output, exit-code classes, CI grammar, and progress behavior remain
unchanged.

## Checkpoints

1. [x] Governance and baseline
   - Confirm the approved committed design, activate this tracker, and record
     CLI output/exit-code plus full CI baseline evidence from v0.5.1.
   - Result: the first implementation checkpoint has an unambiguous, green
     behavioral baseline.
2. [x] Direct command mapping
   - Map parsed interactive arguments directly to `control::Command`; remove
     the duplicate CLI command vocabulary while retaining version and CI-only
     dispatch.
   - Result: every ordinary interactive command reaches control without a
     parallel CLI representation.
3. [x] Unified execution and rendering
   - Consolidate ordinary control execution and exhaustive result rendering;
     remove redundant command-family handlers while preserving output and
     error classification.
   - Result: one CLI adapter path executes and renders all ordinary control
     commands.
4. [x] Deployment and CI execution
   - Consolidate event-capable deployment execution and restricted CI branch
     deployment routing; prove the existing interactive and CI contracts.
   - Result: all deployment entry points share one CLI execution path without
     changing event, progress, or SSH behavior.
5. [ ] Operational regression and closure
   - Synchronize implemented documentation and run the required regression
     ladder before closing the iteration.
   - Result: living documentation reflects the consolidated adapter and the
     public CLI contract remains proven.

## Acceptance Criteria

- Every non-version interactive command maps directly to one `control::Command`;
  no duplicate CLI command enum remains.
- One exhaustive result-rendering path covers every `CommandResult` variant;
  deployment and CI branch deploy retain their event-capable CLI execution path.
- Existing CLI syntax, stdout, non-TTY stderr, verbose output, TTY animation,
  error wording, and exit-code classes remain unchanged.
- Doctor retains diagnostic report rendering for failed checks and database-open
  failures before returning the existing failure class.
- Control integration tests remain CLI- and terminal-free; focused CLI tests
  cover command mapping, rendering policy, and each command family's contract.
- Each code checkpoint passes `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`.
- Environment-dependent OCI and disposable-host checks record their actual
  PASS/FAIL/SKIP state and are never called green when unavailable.

## Blockers

- None.

## Validation Evidence

- Checkpoint 1 (governance and baseline): the approved design is committed as
  `5b0f564`, the roadmap schedules v0.5.2 after completed v0.5.1 and before
  v0.6, and this tracker is the sole active tracker. On `5b0f564`, `cargo fmt
  --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo
  test --all-features` (191 library tests, 9 binary tests, and all integration
  tests), `cargo build --workspace --release`, and markdown-link validation
  passed. `bash -n` passed. The three ignored OCI tests remain skipped because
  `podman` is not installed; `shellcheck` and `shfmt` are unavailable locally.
  Checkpoint 2 is the first pending implementation checkpoint.
- Checkpoint 2 (direct command mapping): `src/cli/args.rs` now maps each
  ordinary parsed command directly to `control::Command` inside
  `InvocationTarget`; only CLI-only version, CI dispatch, and the existing
  missing deploy option remain adapter targets. The dispatcher consumes that
  control command directly, including branch/image deploy selection. New unit
  tests cover representative direct mappings, deploy selection, and
  adapter-only targets; all 73 CLI regressions pass. `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test
  --all-features`, and `cargo build --workspace --release` passed. The three
  OCI tests remain ignored because this host has no `podman`. Checkpoint 3 is
  the first pending implementation checkpoint.
- Checkpoint 3 (unified execution and rendering): `src/cli/mod.rs` now owns
  ordinary control execution, `ControlError` conversion, and one exhaustive
  `CommandResult` renderer. Redundant application, system, exposure,
  reconciliation, and diagnostics handler modules were removed; the existing
  progress-aware deployment handlers delegate their final rendering to that
  renderer. Doctor still renders reports for unhealthy checks and database-open
  failures before returning the prior failure class. The 73 binary CLI
  regressions, including stdout, stderr, and exit-code contracts, pass; focused
  renderer coverage preserves the unhealthy-doctor error class. `cargo fmt
  --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo
  test --all-features`, and `cargo build --workspace --release` passed. The
  three OCI tests remain ignored because this host has no `podman`. Checkpoint 4
  is the first pending implementation checkpoint.
- Checkpoint 4 (deployment and CI execution): `src/cli/mod.rs` now owns one
  event-capable execution path for image deploy, branch deploy, rollback, and
  restricted CI branch deploy. The redundant deployment handler module was
  removed, while the CI dispatcher preserves its existing SSH grammar and
  routes its validated branch request to the shared control command path. A new
  binary CLI regression proves the CI deployment output and non-TTY progress
  contract; all 74 binary CLI regressions pass. `cargo fmt --check`, `cargo
  clippy --all-targets --all-features -- -D warnings`, `cargo test
  --all-features`, `cargo build --workspace --release`, and markdown-link
  validation passed. The three OCI tests remain ignored because this host has
  no `podman`. Checkpoint 5 is the first pending implementation checkpoint.
