# Interface-Neutral Execution

**Status:** approved on 2026-08-31

## Purpose

Move command execution out of the CLI presentation layer into a concrete,
synchronous library boundary that can be called by the existing CLI and later by
a TUI or local HTTP adapter. Add semantic deployment events and an isolated
concurrent CLI renderer for animated terminal progress.

The executable must remain useful and behaviorally identical after every
checkpoint. This is a refactor of ownership, not a behavior change.

## Fixed Decisions

1. The control boundary is synchronous and concrete: commands, typed results,
   typed errors, and an optional event callback. It is not a generic service
   framework, and no trait hierarchy, factory, plugin, or dependency injection
   is introduced.
2. The executor owns immutable host configuration, never a persistent
   `rusqlite::Connection`. Every execution that needs SQLite acquires the
   correct database-wide lock, opens one connection, performs one command,
   closes the connection, and releases the lock.
3. Restore keeps its existing exclusive database-wide lock path; normal
   commands keep shared locking. Lock contention semantics do not change.
4. Existing-Application mutations remain coordinated by the use-case-owned
   per-Application kernel lock. Unrelated Applications remain independent; no
   global mutation queue is introduced (ADR-0005 unchanged).
5. No daemon or resident control plane is introduced. The boundary is in-process
   and on-demand (ADR-0001 unchanged). HTTP and TUI are future adapters and are
   not implemented in this iteration.
6. The control boundary returns typed data. It never calls `println!` or
   `eprintln!`, never detects a TTY, and never chooses process exit codes. Clap
   parsing, text rendering, colors, verbose policy, and exit codes remain in
   `src/cli/`.
7. Workflow progress uses closed semantic enums (for example
   `StepStarted { step: PullImage }`). Free-form diagnostics may carry text.
   Every long blocking deployment operation emits matched start/completion
   events around its real boundary. UI labels and formatting live only in the
   interface layer.
8. Event delivery is observational: a dropped or failed progress consumer never
   changes command success, compensation, or persisted state. Failure evidence
   uses typed `DeploymentFailureCode`, never rendered strings.
9. Spinner animation must tick while synchronous execution blocks, so the CLI
   may use one standard-library thread and channel. Command execution and use
   cases remain synchronous; no async runtime is added.
10. No SQLite schema migration and no new production dependency are expected.
    Any exception requires an explicit approved revision of this design.

## Non-Goals

- No daemon, HTTP server, HTTP client, transport protocol, authentication, or
  wire DTOs.
- No TUI implementation.
- No scheduler, global mutation queue, operation table, persisted event stream,
  cancellation, or retry framework.
- No global serialization of unrelated Applications.
- No SQLite schema change.
- No compatibility layer once the temporary direct CLI execution path is
  removed.

## Acceptance Scenarios

- Every stateful command (systems, import, list, history, status, start, stop,
  deploy by image and branch, rollback, visibility, reconcile, doctor, backup,
  restore, restricted CI dispatch) executes through the control boundary; the
  CLI only maps arguments, renders results and events, and selects exit codes.
- An integration test executes every command family directly through the
  library without Clap or terminal output and receives typed results, errors,
  and semantic events.
- Existing CLI syntax, stdout, non-TTY stderr, and exit-code classes (1 failure,
  2 usage, 3 not found, 4 conflict, 5 external) remain unchanged.
- Same-Application mutations still conflict explicitly; unrelated Applications
  proceed independently; database restore still excludes normal access across
  processes; no SQLite transaction spans an external effect.
- A callback that ignores events and one that collects events produce the same
  result, persisted state, external effects, and error.
- Interactive deployment shows an animated current-step spinner on a TTY;
  redirected and verbose output remain deterministic with no cursor-control
  sequences.
- After the boundary is complete, no use-case, control, adapter, or config code
  writes to the terminal.
- All required Rust, documentation, and applicable disposable-host checks pass.

## Checkpoint Order

1. System vertical slice: first commands through the control boundary.
2. Catalog and query slice: import, application list, deployment history.
3. Runtime lifecycle slice: status, start, stop with real observation effects.
4. Exposure and reconciliation slice: visibility and reconcile with configured
   Caddy paths.
5. Semantic deployment events: complete UI-neutral event vocabulary for deploy
   and rollback, replacing presentation-bearing progress.
6. Deployment and CI slice: deploy, rollback, and restricted CI dispatch through
   the boundary.
7. Diagnostics and database slice: typed doctor report, backup/restore through
   the boundary; CLI no longer owns execution.
8. Concurrent CLI renderer: animated TTY progress, deterministic non-TTY output.
9. Operational regression and closure: documentation synchronization and full
   disposable-host regression.

Governance and baseline recording precede checkpoint 1 and are part of the
iteration-establishing commit. Each checkpoint is independently green, updates
the active tracker and only implemented documentation, and does not begin the
next checkpoint early. Disposable-VM regression is required at checkpoint 6
(deployment dispatch cutover) and at closure.
