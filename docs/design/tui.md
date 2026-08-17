# Design - Terminal User Interface

**Status:** approved design for v0.4 after reconciliation and its required VM
regression. It does not describe implemented behavior. Execution and progress
live only in [`../iterations/current-iteration.md`](../iterations/current-iteration.md).

## Objective

Add an opt-in local terminal interface, `pneuma tui`, for the same operator
workflows offered by the CLI. The TUI lets an operator browse Systems and
Applications, inspect an Application's configuration, history, Releases, and
RuntimeInstances, and run approved operator actions without bypassing existing
use cases or security boundaries.

The existing non-interactive CLI remains the stable automation interface. The
TUI is a local operator interface, not a daemon, API, or replacement for CI.

## Fixed Decisions

- Ratatui renders the interface through Crossterm.
- `pneuma tui` is opt-in and rejects non-terminal stdin or stdout before changing
  terminal state.
- The CLI's commands, exit behavior, stdout, stderr, and CI dispatcher remain
  compatible.
- The TUI provides System create/list/show; Application import/list/status,
  start, stop, image and branch deployment, visibility change, and deployment
  history; deployment rollback; database backup/restore; doctor; and version.
- The restricted SSH-only `ci dispatch` command is not exposed by the TUI. A
  local terminal must not bypass its `SSH_ORIGINAL_COMMAND` security boundary.
- Long operations run as one background job at a time. The UI remains responsive
  and renders progress, errors, and completion events.
- An active effectful job prevents normal TUI exit and conflicting mutations.
  This avoids terminating the process during an external effect.
- SQLite refreshes are read-only. External runtime observation occurs only after
  an explicit operator refresh and does not persist an observation.
- No TUI preferences, selections, or layout state are persisted. This design
  requires no migration.

## Interface

The top-level tabs are self-contained and retain local selection state:

| Tab | Content | Primary actions |
|---|---|---|
| Systems | System table and selected System details, including its Applications. | Create System; open selected Application. |
| Applications | Global Application catalog with System, desired state, deployment, and exposure summaries. | Filter; open selected Application. |
| Application | Selected Application workspace with nested Overview, Deployments, Releases, and Runtime tabs. | Status, start, stop, deploy, visibility, rollback. |
| Import | Remote Git URL, optional System, and manifest-path form. | Import Application. |
| Maintenance | Doctor output, database backup/restore forms, and version. | Run doctor, backup, restore. |

The Application workspace contains:

- **Overview:** identity, System, source, delivery, runtime/health contract,
  Exposure state and diagnostics, and active Deployment.
- **Deployments:** activation history, source revision, type, status, active
  marker, timestamps, and failure evidence.
- **Releases:** immutable artifact reference, repository, digest, creation time,
  and related deployment information.
- **Runtime:** active and historical RuntimeInstances, logical and external
  identities, loopback endpoint, desired/logical/observed states, last
  observation, and retirement evidence.

`Tab` and `Shift-Tab` move between top-level tabs; arrow keys select rows;
`Enter` opens the selected System or Application or submits the focused form.
The footer displays applicable shortcuts, selected Application, and job state.
`q` or `Esc` closes a modal or exits only when no effectful job is active.

Destructive or externally visible actions use a confirmation or input modal.
Database restore requires a prominent destructive confirmation.

## Read Models

The TUI consumes use cases and never contains SQL, Podman, Caddy, Git, or
filesystem orchestration. New presentation-neutral read models are required:

- `SystemDashboardItem`: System with Application count.
- `ApplicationDashboardItem`: Application with System name, deployment,
  exposure, and runtime summary.
- `ApplicationDetails`: Application identity and intent, source, delivery,
  runtime/health configuration, Exposure, active Deployment/Release, and active
  RuntimeInstance.
- `DeploymentHistoryItem`: Deployment, Release artifact, active marker, and
  failure evidence.
- `ReleaseSummary`: Release artifact and associated deployment information.
- `RuntimeInstanceSummary`: RuntimeInstance identity, endpoint, lifecycle,
  observation, diagnostics, and retirement state.
- `ReadOnlyRuntimeObservation`: an external Podman observation that is returned
  without writing SQLite.

These are read projections, not new aggregates or tables. Existing Application,
Release, Deployment, Exposure, and RuntimeInstance domain models remain the
source types. Store adapters own SQL and row mapping; use cases compose
projections and preserve error context.

## Shared Operator Workflows

The current CLI owns several pieces of orchestration that a second interface
must not duplicate. Extract them into explicit use cases with caller-supplied
host configuration:

- remote Git import checkout creation and cleanup;
- doctor diagnostics;
- database backup and restore;
- deploy-by-image, deploy-by-branch, and rollback wrappers that forward progress
  events.

`main.rs` remains responsible for Clap parsing, environment/configuration
loading, and dispatch. The TUI supplies the same explicit paths and configuration
to shared workflows. This is the demonstrated second interface that justifies
moving those workflows out of the CLI entrypoint.

The existing CLI `app status` continues to persist its resulting observation.
The dashboard instead reads persisted state by default. A new explicit
read-only observation use case supports runtime refresh without turning screen
polling into a write operation.

## Jobs And Progress

The application remains synchronous. Ratatui's event loop uses Crossterm polling
and standard-library channels; no async runtime is introduced.

When an operator starts an effectful action, the TUI creates one worker thread.
The worker opens its own SQLite connection from the configured database path and
sends typed started, progress, completed, and failed events to the UI thread.
The UI connection is never shared across threads. After completion, the TUI
reloads only the affected dashboard projections.

Deploy wrappers forward existing deployment lifecycle progress. Other jobs emit
coarse stages around their existing synchronous workflows. A worker never holds
a SQLite transaction across Git, Podman, systemd, Caddy, or HTTP work.

## Terminal Safety

Terminal setup enables raw mode, enters the alternate screen, and hides the
cursor only after TTY validation. A terminal guard restores the cursor, screen,
and raw mode on every normal return or error path. Cleanup failure must not hide
the primary operation error.

The TUI avoids panics after terminal setup. SIGKILL and host termination cannot
reliably restore terminal state; operations documentation must retain a normal
terminal-reset recovery instruction.

## Testing

- Test tab navigation, selection, forms, job-state reduction, and rendering with
  `ratatui::backend::TestBackend`.
- Test new dashboard projections through library integration tests using SQLite
  fixtures.
- Test shared operator workflows with the existing fake command harness for
  Podman, systemd, Caddy, curl, and Git.
- Add a binary-level test confirming that `pneuma tui` rejects non-TTY input and
  output before terminal setup.
- Keep terminal restoration evidence in an optional PTY or configured disposable
  VM test; do not mark unavailable terminal or rootless-Podman checks as passed.

## Implementation Order

1. Add and test dashboard read projections for Systems, Applications, details,
   Deployments, Releases, and RuntimeInstances.
2. Extract shared import, diagnostics, database, and progress-enabled deployment
   workflows from `main.rs` into use cases without changing CLI behavior.
3. Add Ratatui/Crossterm, TTY validation, terminal guard, read-only navigation,
   and rendering tests.
4. Add forms, confirmations, and serialized background jobs for all approved
   operator actions.
5. Complete terminal safety, fake-command integration, configured environment,
   and full CI validation.

## Non-goals

- No HTTP API, web interface, daemon, remote agent, or multi-user terminal
  access.
- No TUI access to `ci dispatch`.
- No automatic deployment, reconciliation, rollback, or external runtime polling.
- No persisted TUI preferences or schema migration.
- No change to SQLite, Podman/systemd, Caddy, Git, or OCI authority boundaries.
