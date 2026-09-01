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

1. [x] Governance and baseline
   - Confirm the approved committed design and record the v0.5.2 behavioral
     baseline with the required Rust CI gates.
   - Result: the first implementation checkpoint has an unambiguous, green
     behavioral baseline.
2. [x] Fallible argument normalization
   - Reject an interactive deploy request without a source during argument
     normalization, before dispatch or any side effect.
   - Cover global `--verbose` and deploy-source grammar with
     `Cli::try_parse_from`, preserving Clap's conflicting-source rejection.
   - Result: no invalid-input variant exists in `InvocationTarget`.
3. [x] Execution organization
   - Give deployment command classification one CLI owner while retaining the
     shared interactive and CI execution path.
   - Result: no deployment output or event sequence changes.
4. [ ] Rendering organization
   - Move application status and lifecycle result text into `output.rs`; remove
     the redundant list-rendering trim and copy from dispatch.
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

- Checkpoint 1 (governance and baseline): the approved design is committed as
  `32a3894`, the roadmap schedules v0.5.3 after completed v0.5.2 and before
  v0.6, and this tracker is the sole active tracker. On `32a3894`, `cargo fmt
  --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo
  test --all-features` (191 library tests, 13 CLI unit tests, and 74 binary CLI
  regressions), and `cargo build --workspace --release` passed. The three OCI
  tests remain ignored because they require configured rootless Podman.
   Checkpoint 2 is the first pending implementation checkpoint.
- Checkpoint 2 (fallible argument normalization): `InvocationTarget::try_from`
  rejects a missing deploy source as `CliError::MissingDeployOption` before
  dispatch, and the `MissingDeployOption` target variant was removed. Grammar
  tests through `Cli::try_parse_from` cover global `--verbose`, image and branch
  sources, the missing-source error, and Clap's conflicting-source rejection; a
  binary regression proves exit 2 with the established error text, no database
  creation, and no external command. `cargo fmt --check`, Clippy with warnings
  denied, all-feature tests (191 library tests, 17 CLI unit tests, 75 binary CLI
  regressions; the three OCI tests remain ignored without rootless Podman), and
  the release build passed. Checkpoint 3 is the next implementation checkpoint.
- Checkpoint 3 (execution organization): one private `deployment_request`
  classifier in `cli/mod.rs` now solely decides event-capable execution and
  feeds the progress renderer, removing the duplicated match and its
  `unreachable!` arm. `execute_control_command` remains the shared interactive
  and CI path, CI still routes validated branch input to
  `Command::DeployBranch`, and child-module visibility was tightened to
  `pub(super)`/private. A unit test covers image, branch, rollback, and ordinary
  classification; interactive image deployment, branch conflict, rollback, CI
  branch deployment, verbose output, and progress coverage are retained. `cargo
  fmt --check`, Clippy with warnings denied, all-feature tests (191 library
  tests, 18 CLI unit tests, 75 binary CLI regressions; the three OCI tests
  remain ignored without rootless Podman), and the release build passed.
  Checkpoint 4 is the next implementation checkpoint.
