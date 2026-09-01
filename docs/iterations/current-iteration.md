# Current Iteration

**Status:** em andamento

**Base:** `e7ade02` (`chore(release): v0.5.0`)

**Approved design:**
[`../designs/interface-neutral-execution.md`](../designs/interface-neutral-execution.md)
(approved 2026-08-31)

## Iteration — Interface-Neutral Execution

Objective: move command execution out of the CLI presentation layer into a
concrete, synchronous library boundary callable by the existing CLI and later by
TUI or HTTP adapters, add semantic deployment events, and add an animated CLI
progress renderer. No daemon, HTTP, or TUI is implemented.

## Checkpoints

1. [x] System vertical slice
   - Add the control module (`Command`, `CommandResult`, `ControlError`,
     `ControlExecutor`, host configuration) and route system
     create/list/show through it.
   - Result: the first non-CLI caller executes real commands through the
     library while CLI output and exit codes remain unchanged.
2. [x] Catalog and query slice
   - Route import, application list, and deployment history through control;
     move workspace configuration and application-name resolution out of CLI.
   - Result: catalog commands prove configured paths and typed collections
     behind the boundary.
3. [x] Runtime lifecycle slice
   - Route status, start, and stop through control with existing observation
     effects and per-Application locking.
   - Result: stateful host operations execute interface-neutrally without
     weakening lock or transaction invariants.
4. [x] Exposure and reconciliation slice
   - Route visibility changes and reconciliation through control with
     control-owned Caddy path configuration.
   - Result: all ordinary application management except deployment uses the
     boundary.
5. [x] Semantic deployment events
   - Replace presentation-bearing deployment progress with closed semantic
     events, matched start/completion boundaries, typed failure codes, and
     typed retirement warnings; add event-capable rollback.
   - Result: deployment reports real blocking operations without UI prose in
     use cases.
6. [x] Deployment and CI slice
   - Route image deploy, branch deploy, rollback, and restricted CI dispatch
     through control; CI translates its validated grammar into the same
     commands as the interactive CLI.
   - Result: interactive and CI deployments share one execution path, proved by
     disposable-host functional E2E.
7. [x] Diagnostics and database slice
   - Convert doctor to a typed report; route doctor, backup, and restore
     through control; remove CLI-owned lock/connection lifetime and remaining
     terminal output outside the interface layer.
   - Result: every stateful command executes through the boundary.
8. [ ] Concurrent CLI renderer
   - Add a CLI-only renderer thread with animated TTY progress, stable verbose
     lines, and deterministic non-TTY output.
   - Result: progress animation exists entirely in the interface layer.
9. [ ] Operational regression and closure
   - Synchronize implemented documentation, remove temporary structure, and run
     full CI plus the complete disposable-host regression.

## Acceptance Criteria

- Every checkpoint meets the acceptance scenarios in the approved design
  without adding its stated non-goals.
- Existing CLI syntax, stdout, non-TTY stderr, and exit-code classes remain
  unchanged throughout the iteration.
- Each code checkpoint has focused tests plus `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`.
- Disposable-host E2E is required at checkpoint 6 and at closure; unavailable
  environments are recorded as skips, never passes.

## Blockers

- None.

## Validation Evidence

- Checkpoint 0 (governance and baseline): the approved design is committed and
  indexed, this tracker is the sole active execution tracker, and the roadmap
  schedules the work as v0.5.1 before v0.6. Baseline on `e7ade02`:
  `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo test --all-features`, `cargo build --workspace --release`,
  and markdown-link validation all passed. The three ignored OCI tests require
  a configured rootless Podman host and remain environment-dependent.
   Disposable-VM baseline: `scripts/dev-vm/test-regression.sh e2e` passed on
   `e7ade02` (fixture cycle, public HTTPS deployment, reboot recovery, rollback,
   and branch-based Git flow); the disposable clone was destroyed afterwards and
   `pneuma-dev-base` was never altered. Checkpoint 1 is the first pending
   implementation checkpoint.
- Checkpoint 1 (system vertical slice): added `src/control/` with `Command`,
  `CommandResult`, `ControlError`, `ControlExecutor`, and `HostConfiguration`;
  the executor acquires the shared database-wide lock and opens one connection
  per command; CLI `system create/list/show` route through it with unchanged
  output and exit codes (new CLI regression test). New `tests/control_system.rs`
  executes create/list/show, missing-system, invalid-name, and database-busy
  scenarios directly through the library. `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release` all
  passed; the three ignored OCI tests remain environment-dependent.
- Checkpoint 2 (catalog and query slice): `HostConfiguration` now owns
  `PNEUMA_WORKSPACE_PATH`; `Command::ImportApplication`, `ListApplications`,
  and `ListDeployments` execute through `ControlExecutor` with typed
  `ApplicationImported`/`Applications`/`ApplicationDeployments` results.
  Application-name resolution moved into
  `use_cases::application::resolve_application` (`ApplicationLookupError`) and
  catalog pairing into `list_application_catalog` (`ApplicationCatalogEntry`);
  the CLI only maps arguments, renders results, and reuses the unchanged error
  vocabulary via `CliError::from_control`. CLI import/list/deployments output
  and exit codes are unchanged (existing CLI regression tests). New
  `tests/control_catalog.rs` executes import, re-import idempotency, catalog
  listing, empty history, missing-application lookup, and local-path rejection
  through the library. `cargo fmt --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test --all-features`, and
  `cargo build --workspace --release` all passed; the three ignored OCI tests
  remain environment-dependent.
- Checkpoint 3 (runtime lifecycle slice): `Command::ApplicationStatus`,
  `ApplicationStop`, and `ApplicationStart` execute through `ControlExecutor`
  with typed `ApplicationStatus`/`ApplicationStopped`/`ApplicationStarted`
  results carrying the use-case `RuntimeObservation`; application resolution
  happens inside the boundary. The per-Application kernel lock and
  intent-before-effect ordering remain owned by
  `use_cases/application/runtime.rs`, so lock and transaction invariants are
  unchanged. CLI status/stop/start map arguments and render observations only;
  messages and exit-code classes are unchanged (`RuntimeStatus`, `RuntimeStop`,
  and `RuntimeStart` map onto the existing `CliError::ApplicationRuntime`
  vocabulary). New `tests/control_lifecycle.rs` executes observation,
  direct-container stop, supervised start recovery of a recreated container,
  missing-application lookup errors, and undeployed `NotDeployed` errors
  through the library without Clap or terminal output. `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release` all
  passed; the three ignored OCI tests remain environment-dependent.
- Checkpoint 4 (exposure and reconciliation slice): `HostConfiguration` now
  owns `PNEUMA_CADDY_MANAGED_PATH` and `PNEUMA_CADDYFILE_PATH`;
  `Command::VisibilitySet` and `Command::Reconcile` execute through
  `ControlExecutor` with typed `ExposureChanged` and `Reconciled` results. The
  CLI renders only; messages and exit-code classes are unchanged, including the
  reconcile invalid-name path mapping onto the existing
  `ReconciliationReadError::ApplicationNotFound` vocabulary. The per-Application
  lock, CAS exposure reservations, and Caddy compensation ordering remain owned
  by the use cases. New `tests/control_exposure.rs` executes missing-application
  lookup, domain-required rejection, a full internal→public materialization
  (typed result, canonical fragment, fake `caddy` validate/reload, external
  health `curl`, persisted `public`/`active` state), and missing/undeployed
  reconcile errors through the library without Clap or terminal output.
  `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo test --all-features`, and `cargo build --workspace
  --release` all passed; the three ignored OCI tests remain
   environment-dependent.
- Checkpoint 5 (semantic deployment events): replaced the presentation-bearing
  progress API with closed `DeploymentEvent`/`DeploymentStep` enums, including
  typed `DeploymentFailureCode` evidence and `RetirementWarning` variants.
  Branch resolution, image resolution and pulling, candidate materialization,
  health checks, promotion, cleanup, and prior-runtime retirement now emit
  matched boundaries around their real operations; rollback has the same
  event-capable path. Deployment use cases no longer write retirement warnings
  to the terminal; the CLI renders event labels and preserves non-verbose
  retirement warnings. Deployment integration tests assert complete OCI and
  branch event sequences, typed failure/warning events, and verbose rollback
  events. `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo test --all-features`, and `cargo build --workspace --release`
  all passed; markdown-link validation and shell syntax checks passed. The three
  ignored OCI tests remain environment-dependent; `shellcheck` and `shfmt` were
  unavailable locally, and no shell files changed.
- Checkpoint 6 (deployment and CI slice): `Command::DeployImage`,
  `DeployBranch`, and `Rollback` execute through `ControlExecutor` with typed
  deployment results, errors, and semantic event delivery; the boundary owns
  public Caddy-path configuration and preserves the CLI's existing output and
  error classification. The restricted CI adapter now translates its validated
  grammar into the same branch-deploy command as the interactive CLI. New
  `tests/control_deployment.rs` directly imports, deploys an image, deploys
  branches (including the CI grammar), and rolls back through the library with
  typed results and collected events. `cargo fmt --check`, `cargo clippy
  --all-targets --all-features -- -D warnings`, `cargo test --all-features`, and
  `cargo build --workspace --release` all passed; the three ignored OCI tests
  remain environment-dependent. Disposable-host E2E passed via
  `scripts/dev-vm/test-regression.sh e2e`: all 45 checks passed, including
  branch deployment and restricted CI dispatch; the disposable clone was
  destroyed afterwards and `pneuma-dev-base` was never altered.
- Checkpoint 7 (diagnostics and database slice): `Command::Doctor`,
  `DatabaseBackup`, and `DatabaseRestore` now execute through `ControlExecutor`;
  doctor returns a typed `DoctorReport`, including a typed database-open failure
  report, while the CLI owns verbose and report rendering. Backup reuses the
  executor-owned connection under the shared lock; restore retains its exclusive
  lock path and returns typed source and pre-restore paths. Diagnostic adapters,
  control, use cases, and configuration no longer write to the terminal. New
  `tests/control_diagnostics_database.rs` executes diagnostics, backup, restore,
  and database-open failure directly through the library; CLI backup/restore
  output remains covered. `cargo fmt --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test --all-features`, and `cargo build
  --workspace --release`, markdown-link validation, and shell syntax checks all
  passed; the three ignored OCI tests remain environment-dependent. `shellcheck`
  and `shfmt` were unavailable locally, and no shell files changed.
