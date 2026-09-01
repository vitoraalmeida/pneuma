# Current Iteration

**Status:** em andamento

**Base:** `5a41884` (`docs: close CLI adapter consolidation iteration`)

**Approved design:**
[`../designs/cli-adapter-integrity.md`](../designs/cli-adapter-integrity.md)
(approved 2026-09-01)

## Iteration - CLI Adapter Integrity

Objective: correct remaining CLI adapter imprecision after v0.5.2. This is a
structural refactor: CLI syntax, text output, exit-code classes, CI grammar, and
progress behavior remain unchanged.

## Checkpoints

1. [ ] Governance and baseline
   - Confirm the approved committed design and record the v0.5.2 behavioral
     baseline with the required Rust CI gates.
   - Result: the first implementation checkpoint has an unambiguous, green
     behavioral baseline.
2. [ ] Fallible argument normalization
   - Reject an interactive deploy request without a source during argument
     normalization, before dispatch or any side effect.
   - Result: no invalid-input variant exists in `InvocationTarget`.
3. [ ] Execution organization
   - Give deployment command classification one CLI owner while retaining the
     shared interactive and CI execution path.
   - Result: no deployment output or event sequence changes.
4. [ ] Rendering organization
   - Move application status and lifecycle result text into `output.rs`.
   - Result: final command text is owned by output functions while dispatch
     retains rendering policy and control flow.
5. [ ] Operational regression and closure
   - Synchronize implemented documentation and run the required regression
     ladder before closing the iteration.
   - Result: living documentation reflects the precise CLI adapter while the
     public contract remains proven.

## Acceptance Criteria

- Missing deploy source fails before CLI dispatch, control, SQLite, or external
  commands, with unchanged visible error text and exit class.
- Image and branch deployment preserve their direct mappings and conflicting
  source options retain Clap rejection.
- One classifier covers image deployment, branch deployment, and rollback;
  interactive and CI branch deployment retain their event-capable path.
- Application status, start, and stop output remain unchanged and are formatted
  by `output.rs`.
- Existing CLI syntax, stdout, non-TTY stderr, verbose output, TTY animation,
  error wording, and exit-code classes remain unchanged.
- Each code checkpoint passes `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`.
- Environment-dependent OCI and disposable-host checks record their actual
  PASS/FAIL/SKIP state and are never called green when unavailable.

## Blockers

- None.

## Validation Evidence

- Pending.
