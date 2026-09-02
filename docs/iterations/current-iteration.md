# Current Iteration

**Status:** em andamento

**Base:** `6227c0e` (`Change events configuration`)

**Approved design:**
[`../designs/cli-operational-robustness.md`](../designs/cli-operational-robustness.md)
(approved 2026-09-02)

## Iteration - CLI Operational Robustness (v0.5.4)

Objective: correct all identified CLI robustness, error-classification,
presentation, bootstrap, and test-organization issues found in the
post-v0.5.3 review. The approved behavior-change table in the design is the
authoritative target for observable corrections.

## Checkpoints

1. [x] Governance and baseline
   - Confirm the approved committed design, the v0.5.4 roadmap entry, the
     docs index, and the queued v0.6 planning reminder; establish the
     behavioral baseline with the required Rust CI gates.
   - Result: the approved design is committed, exactly one active tracker
     exists, and the baseline is green at `6227c0e`.
2. [ ] Observer and progress isolation
   - Make progress output best effort; a failed stderr write cannot unwind
     through deployment execution.
3. [ ] Explicit presentation vocabulary
   - Remove stable CLI dependence on domain enum `Debug` formatting.
4. [ ] Total doctor rendering
   - Preserve captured doctor diagnostics and render every publicly
     constructible outcome without panic.
5. [ ] Lock failure classification
   - Distinguish lock infrastructure failure (exit 1) from real contention
     (exit 4).
6. [ ] Nested deployment classification
   - Classify deployment failures by typed semantic cause.
7. [ ] Remaining classification audit
   - Complete exhaustive CLI error semantics.
8. [ ] Strict host environment contract
   - Fail fast on unreadable, malformed, duplicate, invalid UTF-8, or
     invalid-variable host environment files.
9. [ ] Invocation boundary coverage
   - Cover adapter-only commands in a dedicated test target.
10. [ ] Shared CLI test support
   - Extract the deployment harness into `tests/cli/support.rs`.
11. [ ] Reconciliation test module
12. [ ] Lifecycle test module
13. [ ] Exposure test module
14. [ ] Deployment test module
15. [ ] Catalog and database modules
16. [ ] Operational regression and closure

## Acceptance Criteria

- Every scenario in the design's corrected behavior table behaves exactly as
  corrected; all other successful stdout/stderr bytes remain unchanged.
- Renderer I/O cannot unwind through control execution; both TTY and non-TTY
  progress contracts are tested.
- Stable CLI text does not depend on domain enum `Debug` formatting.
- Doctor rendering is total and preserves captured diagnostics.
- Lock open/acquire failures exit 1 while real contention remains exit 4.
- A malformed or unreadable host environment file fails startup atomically
  with one contextual `error:` line; a missing file continues to boot.
- CLI integration tests are organized into capability modules with shared
  support and no duplicated harness.
- Each code checkpoint passes `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`.
- Environment-dependent OCI and disposable-host checks record their actual
  PASS/FAIL/SKIP state with reasons and are never called green when
  unavailable.

## Blockers

- None.

## Validation Evidence

- Checkpoint 1 (governance and baseline): the approved design is committed in
  this checkpoint, the roadmap marks v0.5.3 completed and schedules v0.5.4
  before v0.6, and this tracker is the sole active tracker. The observer seam
  refactor committed as `6227c0e` is retained as the baseline with its
  ownership recorded in the design. Baseline gates on `6227c0e`: `cargo fmt
  --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release` all
  passed, plus markdown-link validation. The three OCI tests remain ignored
  on this host because rootless Podman is not configured. Checkpoint 2 is the
  first pending implementation checkpoint.
