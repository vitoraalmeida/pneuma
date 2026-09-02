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
2. [x] Observer and progress isolation
   - Make progress output best effort; a failed stderr write cannot unwind
     through deployment execution.
   - Result: `write_stderr_line` gives `log_verbose` and stable progress
     locked-stderr best-effort writes; the animated renderer writes through a
     sink with ignored errors, so presentation failures cannot abort a
     deployment. A binary regression proves deployment succeeds and persists
     when stderr rejects writes (read-only `/dev/null`), and a Linux PTY unit
     test proves the TTY path emits `Deploying`, multiple spinner frames, and
     the clear-line control bytes; stable and verbose text remain unchanged.
3. [x] Explicit presentation vocabulary
   - Remove stable CLI dependence on domain enum `Debug` formatting.
   - Result: `output.rs` now owns exhaustive label functions for desired
     runtime state, observed runtime state (keeping the
     `Unknown { status: "..." }` representation), deployment type, deployment
     status, and visibility; history rows, progress state changes, status and
     lifecycle text, and verbose visibility logs reuse them. No `{:?}`
     formatting remains in the CLI, and stable text is byte-for-byte
     unchanged.
4. [x] Total doctor rendering
   - Preserve captured doctor diagnostics and render every publicly
     constructible outcome without panic.
   - Result: failed Git/Podman/Caddy checks render
     `command failed (<detail>)` with the generic line retained when detail
     is exactly `command failed`, and `ActiveOciImages(Passed)` renders
     `✓ Active OCI images: <detail>` instead of panicking; check ordering,
     health calculation, summary, stderr error, and exit 1 are unchanged.
5. [x] Lock failure classification
   - Distinguish lock infrastructure failure (exit 1) from real contention
     (exit 4).
   - Result: every `ApplicationLockError::Open`/`Acquire` wrapper in deploy,
     branch deploy, rollback, runtime lifecycle, and visibility classification
     is `Failure`, every `ApplicationBusy` wrapper stays `Conflict`, and
     `ReconciliationReadError::OperationLock` is `Failure` since reconciliation
     reports contention as a successful `Deferred`; error wording and source
     chains are unchanged.
6. [x] Nested deployment classification
   - Classify deployment failures by typed semantic cause.
   - Result: the CLI classifiers gained `classify_create_release`,
     `classify_create_deployment`, `classify_deploy_release`,
     `classify_deployment_failure_code`, `classify_candidate_cleanup`, and
     `classify_transition_deployment`; nested OCI, branch, and rollback errors
     delegate through them, nested missing resources are `NotFound`, nested
     state conflicts are `Conflict`, cleanup systemd/Podman divergence is
     `External`, cleanup persistence is `Failure`, and every
     `DeploymentFailureCode` maps to either `External` (exit 5) or generic
     `Failure` (exit 1) with no string matching or downcasting. Messages and
     source chains are unchanged.
7. [x] Remaining classification audit
   - Complete exhaustive CLI error semantics.
   - Result: every remaining wildcard match in `cli::error` was replaced with
     exhaustive typed matches; the approved mappings now hold —
     `ImportError::SystemRequired` and a missing default branch are `Usage`,
     missing source/delivery configuration and a required exposure domain are
     `Conflict`, persisted invalid visibility (and other invalid persisted
     values) are `Failure`, and a non-loopback observed endpoint is
     `External`. Messages, source chains, and the numeric class definitions
     are unchanged.
8. [x] Strict host environment contract
   - Fail fast on unreadable, malformed, duplicate, invalid UTF-8, or
     invalid-variable host environment files.
   - Result: `src/host_environment.rs` owns startup configuration — the file
     path comes from `PNEUMA_HOST_ENVIRONMENT_FILE` (default
     `/etc/pneuma/environment`), only `NotFound` is ignored, bytes are
     validated as UTF-8, entries must be `NAME=VALUE` with valid variable
     names and no NUL bytes, duplicates are rejected with both line numbers,
     and the whole file validates before any assignment; caller environment
     values win, XDG/D-Bus derivation is unchanged, a nonempty `HOME` or
     `PNEUMA_QUADLET_DIR` is required, and an empty `HOME` no longer derives a
     bogus Quadlet directory. Startup failures exit 1 with empty stdout and
     one contextual `error:` line before argument parsing, creating no
     database and running no external command.
9. [x] Invocation boundary coverage
   - Cover adapter-only commands in a dedicated test target.
   - Result: `tests/cli_invocation.rs` owns the adapter-only invocation
     regressions — the unknown-command usage error moved here from the CLI
     deployment target, exact-output `pneuma version` and CI-dispatched
     `version` tests prove both paths print `pneuma <release>` with empty
     stderr, the missing `SSH_ORIGINAL_COMMAND` dispatch fails with exit 2 and
     the exact `error: SSH_ORIGINAL_COMMAND not set` line, and every
     version/dispatch scenario asserts the configured database path is never
     created.
10. [x] Shared CLI test support
   - Extract the deployment harness into `tests/cli/support.rs`.
   - Result: `DeploymentEnvironment` (with its `OciFailure` enum, constructors,
     and deploy/lifecycle/reconcile helpers), the fake `podman`/`systemctl`/
     `caddy`/`curl` installers, the Git and one-shot HTTP helpers, and the
     common process assertions moved to `tests/cli/support.rs` with `pub(super)`
     visibility only where sibling modules need access; fake command
     implementations and `git`/`read_request`/`unique_suffix` stay private.
     Exposure helpers (`run_visibility_command*`, `assert_exposure_state`) and
     `current_runtime_states` remain local to their future capability modules.
     Because Rust resolves child modules of a test-crate root in its own
     directory, the target root moved mechanically from `tests/cli.rs` to
     `tests/cli/main.rs`; the target name `cli`, all 84 tests, and every
     assertion are unchanged.
11. [x] Reconciliation test module
    - Move all reconciliation scenarios into `tests/cli/reconciliation.rs`.
    - Result: the seventeen reconcile scenarios (no-op, deferred, runtime
      repair/rematerialization/health-failure/divergence, and public and
      internal exposure reconciliation) moved verbatim into the new module
      with only import adjustments; `PermissionsExt` and `ApplicationLock`
      left `tests/cli/main.rs` with them since no residual test uses them.
      No scenario, assertion, or helper was rewritten and nothing was
      duplicated.
12. [x] Lifecycle test module
    - Move status/start/stop and removed-container scenarios into
      `tests/cli/lifecycle.rs`; move `current_runtime_states` with them.
    - Result: the thirteen status/start/stop and removed-container scenarios
      (desired/observed status report, idempotent stop/start persistence,
      non-deployed and unknown application failures, runtime from a
      non-succeeded deployment ignored, failed start desired-state retention,
      removed-container deployment guidance, stop/start cycle after quadlet
      removal, missing-after-stop preservation, OCI start after container
      removal, no reconciliation of a recreated stable-name container, no
      external id CAS attempts, and endpoint/timestamp preservation) moved
      verbatim into the new module with the `current_runtime_states` helper,
      which only those tests used; `rusqlite::OptionalExtension` left
      `main.rs` with them since no residual test uses it. No scenario,
      assertion, or helper was rewritten and nothing was duplicated.
13. [x] Exposure test module
    - Move visibility, compensation, invalid visibility, and legacy `expose`
      scenarios into `tests/cli/exposure.rs`; keep exposure helpers local.
    - Result: the nine exposure scenarios (public/internal visibility toggle,
      idempotent internal set without a domain, public intent persisted as
      failed without an active runtime, domain-required rejection before
      external effects, non-loopback observed endpoint as external, failed
      public health restoring the previous fragment, lost completion CAS
      restoration, legacy `expose` usage, and unknown-visibility rejection)
      moved verbatim into the new module together with the
      `run_visibility_command*` family and `assert_exposure_state`, which no
      residual test uses; `Output` left `main.rs` with them since nothing
      residual needs it. No scenario, assertion, or helper was rewritten and
      nothing was duplicated.
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
- Checkpoint 2 (observer and progress isolation): stderr line writes in the
  CLI progress path are now best effort through `cli::shared::write_stderr_line`
  (locked stderr, ignored errors); `log_verbose` routes through it, and the
  animated renderer renders into a `Write` sink with ignored `writeln!` errors
  while keeping spinner frames, timing, event ordering, TTY selection, and
  final output unchanged. The binary regression
  `deployment_continues_when_non_tty_stderr_rejects_progress_writes` proves a
  deployment succeeds and persists `desired_runtime_state` when child stderr
  is a read-only `/dev/null` (EBADF writes), and
  `animated_tty_progress_emits_lifecycle_text_frames_and_clear_bytes` proves
  the TTY path over a `libc::openpty` PTY. Focused tests passed (progress
  unit tests, the new binary regression, verbose lifecycle stderr,
  deployment_from_oci, deployment_from_revision, control_deployment), then
  `cargo fmt --check`, Clippy with warnings denied, all-feature tests, and
  the release build passed. Checkpoint 3 is the next implementation
  checkpoint.
- Checkpoint 3 (explicit presentation vocabulary): `cli::output` gained
  `desired_runtime_state_label`, `observed_runtime_state_label` (preserving
  the exact `Unknown { status: "..." }` form), `deployment_type_label`,
  `deployment_status_label`, and `visibility_label`. `runtime_status`,
  `lifecycle_outcome`, and `deployment_history` rows render those labels
  instead of `Debug`; progress state changes reuse
  `deployment_status_label`; the verbose visibility log reuses
  `visibility_label`; `visibility_change` keeps its exact prior bytes.
  Unit tests cover every variant label, the unknown representation, and
  exact history rows; `grep '{:?}' src/cli` finds no remaining domain-enum
  `Debug` formatting. Focused tests (output and progress unit tests,
  `reports_desired_and_observed_state_after_deployment`,
  `lists_deployments_for_a_deployed_application`) passed, then `cargo fmt
  --check`, Clippy with warnings denied, all-feature tests, and the release
  build passed. Checkpoint 4 is the next implementation checkpoint.
- Checkpoint 4 (total doctor rendering): `format_command_availability` now
  renders failed Git/Podman/Caddy checks as `command failed (<detail>)`,
  keeping the generic line when the captured detail is exactly `command
  failed`; unavailable-command wording is unchanged. The
  `ActiveOciImages(Passed)` `unreachable!` panic was replaced by
  `✓ Active OCI images: <detail>`. A unit test renders every publicly
  constructible doctor outcome without panicking and asserts the exact
  detailed, generic, and collective-success lines; binary regressions with a
  failing fake `git` prove the detailed stdout line plus unchanged stderr
  error and exit 1, and the generic fallback when the command reports no
  detail. `architecture.md` now documents the detailed doctor failure lines.
  Focused tests, `cargo fmt --check`, Clippy with warnings denied,
  all-feature tests, markdown-link validation, and the release build passed.
  Checkpoint 5 is the next implementation checkpoint.
- Checkpoint 5 (lock failure classification): the CLI classifiers now split
  lock wrappers — `ApplicationLock` (open/acquire) wrappers for deploy,
  branch deploy, rollback, runtime lifecycle, and visibility change classify
  as `Failure` (exit 1) while `ApplicationBusy` wrappers remain `Conflict`
  (exit 4); `ReconciliationReadError::OperationLock` moved to `Failure`
  because reconciliation already reports contention as a successful
  `Deferred` result. Messages and source chains are unchanged. Unit tests
  cover every CLI-visible lock wrapper (open and acquire) with matching
  `ApplicationBusy` cases; the binary regression
  `deploy_fails_with_exit_1_when_the_application_lock_cannot_be_opened`
  creates a directory at the lock sidecar path so open fails deterministically
  with the lock-path diagnostic, and the existing contention regression now
  also asserts exit 4. Focused tests, `cargo fmt --check`, Clippy with
  warnings denied, all-feature tests, and the release build passed.
  Checkpoint 6 is the next implementation checkpoint.
- Checkpoint 6 (nested deployment classification): `cli::error` classifies
  nested deployment failures through typed helpers — release creation,
  deployment creation, release execution, transition divergence, and candidate
  cleanup each map to their semantic class, and all 18 `DeploymentFailureCode`
  stages split into 8 external codes (exit 5) and 10 generic codes (exit 1).
  Nested missing resources are `NotFound`, nested state conflicts are
  `Conflict`, and the nested-`DeploymentFailed` source chain stays reachable.
  Binary regressions prove exit 5 with the persisted code for a fake `systemctl`
  start failure, a failing internal TCP health check, a rejecting fake `caddy`
  (new `PNEUMA_FAKE_CADDY_FAILURE` hook), and a 500 external health check; the
  stale exit-1 expectation in `deploy_fails_when_systemctl_start_fails` was
  corrected to exit 5 per the approved behavior table. Focused tests (error
  unit tests, the full `cli` target, `deployment_execute_release`), `cargo fmt
  --check`, Clippy with warnings denied, all-feature tests (3 OCI tests remain
  ignored without rootless Podman), and the release build passed. Checkpoint 7
  is the next implementation checkpoint.
- Checkpoint 7 (remaining classification audit): `cli::error` classifications
  are now exhaustive — `classify_deploy_branch`, `classify_deploy_oci`,
  `classify_remote_import`, `classify_runtime_lifecycle`,
  `classify_exposure_change`, `classify_reconciliation_read`, and the
  `SystemShow` and `DatabaseError` matches enumerate every variant with the
  approved semantics: import missing required system (exit 2), missing
  default branch (exit 2), missing source/delivery configuration (exit 4),
  exposure domain required (exit 4), persisted invalid visibility and other
  invalid persisted values (exit 1), non-loopback observed endpoint (exit 5),
  and stores/persistence/lock infrastructure (exit 1). Unconverged
  reconciliation state keeps its generic `Failure` class. Messages, source
  chains, and the 1–5 class definitions are unchanged. Unit coverage gained
  ten representative cases including every corrected mapping; binary
  regressions prove exit 2 with the `system is required` diagnostic for a
  manifest import without a system, exit 4 for public exposure without a
  domain, and exit 5 for a non-loopback observed endpoint via a new
  `PNEUMA_FAKE_PODMAN_PORT` hook; the persisted-invalid-visibility scenario
  is unreachable at the binary level because the schema CHECK constraint
  forbids it, so it is covered by unit classification. Focused tests (error
  unit tests, the full `cli` target, control_exposure, application_import),
  `cargo fmt --check`, Clippy with warnings denied, all-feature tests (3 OCI
  tests remain ignored without rootless Podman), and the release build
  passed. Checkpoint 8 is the next implementation checkpoint.
- Checkpoint 8 (strict host environment contract): bootstrap moved to
  `src/host_environment.rs::configure_startup_environment`; the host
  environment file is read through `PNEUMA_HOST_ENVIRONMENT_FILE` (default
  `/etc/pneuma/environment`), only `NotFound` is tolerated, and unreadable
  files, invalid UTF-8, missing separators, empty or invalid variable names,
  NUL bytes in values, and duplicate variables each fail startup with one
  contextual `error:` line, exit 1, empty stdout, no database creation, no
  external command, and before argument parsing. The whole file validates
  before any entry is applied, caller values override file values, XDG/D-Bus
  derivation is unchanged, a nonempty `HOME` or `PNEUMA_QUADLET_DIR` is
  required after derivation, and an empty `HOME` no longer derives a bogus
  Quadlet directory. Coverage: five parse unit tests plus binary regressions
  for a missing file booting normally, an unreadable path (directory), invalid
  UTF-8, valid entries applying the file-provided `PNEUMA_DATABASE_PATH`,
  caller precedence, malformed lines with the line number, invalid names, NUL
  bytes, duplicates with both line numbers, no partial application after a
  late parse failure, and the HOME/Quadlet requirement. Focused tests
  (`--lib host_environment`, `--test host_environment`), `cargo fmt --check`,
  Clippy with warnings denied, all-feature tests (3 OCI tests remain ignored
  without rootless Podman), and the release build passed. Checkpoint 9 is the
  next implementation checkpoint.
- Checkpoint 9 (invocation boundary coverage): the new `cli_invocation` test
  target owns the adapter-only invocation paths. The unknown-command usage
  regression moved there unchanged, the direct and CI-dispatched version tests
  assert the exact `pneuma <CARGO_PKG_VERSION>` stdout with empty stderr and
  that the configured database path is never created, and the missing
  `SSH_ORIGINAL_COMMAND` regression asserts exit 2 with exactly
  `error: SSH_ORIGINAL_COMMAND not set` on stderr, an empty stdout, and no
  database creation. Focused tests (`cargo test --test cli_invocation`, 4
  passed), `cargo fmt --check`, Clippy with warnings denied, all-feature tests
  (3 OCI tests remain ignored without rootless Podman), and the release build
  passed. Checkpoint 10 is the next implementation checkpoint.
- Checkpoint 10 (shared CLI test support): the deployment harness moved to
  `tests/cli/support.rs` — `DeploymentEnvironment` and `OciFailure`, the fake
  executable installers (private), Git (`git`, `initialize_repository`,
  `create_repository_from_fixture`, `fixture_path`), one-shot HTTP
  (`respond_once`, private `read_request`), process helpers (`run_pneuma`,
  `run_pneuma_env`, `executable_path`, `make_executable`, `wait_for_file`,
  `wait_for_child`), common assertions (`assert_command_succeeded`,
  `assert_identifier_line`), and `temporary_database_path`/
  `temporary_workspace_path`/`unique_suffix`. Items used by test bodies use
  `pub(super)`; nothing is duplicated. Since a test-crate root cannot resolve
  children in a same-named subdirectory, `tests/cli.rs` became
  `tests/cli/main.rs` — the `cli` target name, the 84 tests, and all assertions
  are unchanged, and `cargo test --test cli` passes. Focused tests (84/84 in
  the `cli` target), `cargo fmt --check`, Clippy with warnings denied,
  all-feature tests (3 OCI tests remain ignored without rootless Podman), and
  the release build passed. Checkpoint 11 is the next implementation
  checkpoint.
- Checkpoint 11 (reconciliation test module): all seventeen reconcile
  scenarios moved mechanically from `tests/cli/main.rs` to
  `tests/cli/reconciliation.rs` (no-op for stopped intent, converged running,
  deferred before external observation, confirmed container recreation
  repair, quadlet rematerialization, divergent rematerialization refusal,
  rematerialized health failure, lost runtime confirmation, canonical quadlet
  restart, divergent recreated container, missing public Caddy fragment,
  failed public exposure via external health and via Caddy rejection, lost
  public confirmation, internal fragment removal, lost removal completion
  CAS, and diverged exposure intent) without rewriting any scenario; the
  module imports `fs`, `Ipv4Addr`/`TcpListener`, `PermissionsExt`, `thread`,
  `ApplicationLock`, `database`, and support's `DeploymentEnvironment`,
  `assert_command_succeeded`, and `respond_once` — the latter two imports and
  `PermissionsExt`/`ApplicationLock` were pruned from `main.rs` because no
  residual test uses them. Focused tests (`cargo test --test cli
  reconciliation::`, 17 passed), the full `cli` target (84/84), `cargo fmt
  --check`, Clippy with warnings denied, all-feature tests (3 OCI tests
  remain ignored without rootless Podman), and the release build passed.
  Checkpoint 12 is the next implementation checkpoint.
- Checkpoint 12 (lifecycle test module): all thirteen status/start/stop and
  removed-container scenarios moved mechanically from `tests/cli/main.rs` to
  `tests/cli/lifecycle.rs` — desired/observed status reporting, idempotent
  stop/start persistence, lifecycle failures for non-deployed and unknown
  applications, ignoring a runtime from a non-succeeded deployment, failed
  start desired-state retention, removed-container deployment guidance, the
  stop/start cycle after quadlet container removal, missing-after-stop
  preservation, starting a verified OCI image after container removal, no
  reconciliation of a container recreated under the stable name, no external
  id CAS attempts, and expected-endpoint/timestamp preservation — together
  with the `current_runtime_states` helper, which only those tests used;
  `rusqlite::OptionalExtension` was pruned from `main.rs` because no residual
  test uses it. No scenario, assertion, or helper was rewritten and nothing
  was duplicated. Focused tests (`cargo test --test cli lifecycle::`, 13
  passed), the full `cli` target (84/84), `cargo fmt --check`, Clippy with
  warnings denied, all-feature tests (3 OCI tests remain ignored without
  rootless Podman), and the release build passed. Checkpoint 13 is the next
  implementation checkpoint.
- Checkpoint 13 (exposure test module): all nine exposure scenarios moved
  mechanically from `tests/cli/main.rs` to `tests/cli/exposure.rs` — the
  public/internal visibility toggle, idempotent internal set without a domain,
  public intent persisted as failed without an active runtime, domain-required
  rejection before external effects, non-loopback observed endpoint as an
  external failure, failed public health restoring the previous fragment,
  lost public completion CAS restoring the fragment, the legacy `expose`
  usage error, and the unknown-visibility rejection — together with the
  `run_visibility_command`, `run_visibility_command_with_curl_status`,
  `run_visibility_command_with_podman_port`, `run_visibility_command_with_options`,
  and `assert_exposure_state` helpers, which no residual test uses; `Output`
  was pruned from `main.rs` because nothing residual needs it. No scenario,
  assertion, or helper was rewritten and nothing was duplicated. Focused
  tests (`cargo test --test cli exposure::`, 9 passed), the full `cli` target
  (84/84), `cargo fmt --check`, Clippy with warnings denied, all-feature tests
  (3 OCI tests remain ignored without rootless Podman), and the release build
  passed. Checkpoint 14 is the next implementation checkpoint.
