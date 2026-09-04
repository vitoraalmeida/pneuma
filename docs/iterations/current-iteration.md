# Current Iteration

**Status:** em andamento

**Version:** v0.5.5 - Terminal User Interface

**Base:** `2c1f47e` (`docs: close VM E2E readiness iteration`)

**Authorization:** approved directly by the repository owner on 2026-09-03.

## Objective

Provide an interactive terminal interface for inspecting and operating one
Pneuma host. The TUI is a Ratatui adapter over the existing synchronous
`ControlExecutor`; it does not create a resident control plane or duplicate
domain, use-case, persistence, or external-effect decisions.

## Fixed Decisions

1. `pneuma tui` is a new explicit interactive command. Existing CLI commands,
   syntax, stdout, stderr, and exit-code behavior remain unchanged.
2. The TUI uses Ratatui with Crossterm as its terminal backend. It is the only
   new interface dependency introduced by this iteration.
3. The TUI is available only on an interactive terminal. When stdin or stdout
   is not a terminal, it fails before opening the database or invoking an
   external command with a usage-class diagnostic.
4. The TUI owns terminal setup, restoration, keyboard handling, screen layout,
   presentation labels, and transient interaction state. `src/control/` stays
   terminal-neutral and synchronous.
5. Every host action maps to an existing typed `control::Command`; no TUI-only
   business operation or second command vocabulary is introduced.
6. Read screens refresh by issuing new control commands on demand. A refresh
   never keeps a SQLite connection or database lock between renders.
7. Mutating actions require an explicit in-TUI confirmation. Deployment and
   rollback show the existing semantic deployment events while the command is
   executing; terminal cleanup runs on success, command failure, and TUI error.
8. The initial TUI covers application catalog, application details and
   deployment history, runtime status, start, stop, reconcile, visibility,
   deploy by branch or digest, and rollback. Import, system administration,
   doctor, backup, restore, and restricted CI dispatch remain CLI-only.

## Non-goals

- No daemon, HTTP API, remote TUI, web interface, authentication, transport,
  multi-host operation, background refresh worker, async runtime, or new
  persistence.
- No changes to deployment, reconciliation, SQLite locking, runtime, Caddy, or
  CI-dispatch semantics.
- No mouse support, configurable themes, saved UI state, shell command runner,
  text editor, or generic terminal abstraction.
- No v0.6 observed-state model or observation-based reconciliation work.

## Checkpoints

1. [x] Establish the TUI command and terminal lifecycle
   - Add `pneuma tui`, Ratatui/Crossterm dependencies, terminal capability
     validation, and a minimal full-screen application shell that always
     restores the terminal.
   - Result: `pneuma tui` now rejects non-interactive streams before host
     configuration, opens a Ratatui/Crossterm shell, and restores raw/alternate
     terminal state through explicit cleanup and `Drop`. Argument, non-TTY, and
     pseudo-terminal normal-exit regressions avoid ANSI-byte assertions.
2. [ ] Read-only catalog and application inspection
   - Render the application catalog, selected application detail, deployment
     history, and on-demand runtime status from typed control results.
   - Define deterministic empty, loading, error, selection, refresh, and quit
     states with keyboard-only navigation.
3. [ ] Confirmed lifecycle and exposure actions
   - Add confirmation flows for start, stop, reconcile, and visibility changes,
     preserving typed control-error classification in user-visible TUI errors.
4. [ ] Deployment and rollback interaction
   - Add branch and digest deployment forms plus rollback confirmation; render
     semantic deployment events and final typed results without changing
     deployment execution or compensation.
5. [ ] Operational regression and closure
   - Synchronize implemented documentation and run the required Rust, markdown,
     shell, and applicable disposable-VM regression ladder.

## Acceptance criteria

- `pneuma tui` opens only on an interactive terminal, restores terminal state
  after every exit path, and leaves the existing CLI contract unchanged.
- An operator can navigate the defined catalog and detail views, refresh data,
  inspect status and history, and see actionable typed errors without a panic.
- Every supported mutation has a visible confirmation step and executes through
  the existing control boundary; no SQLite transaction or lock outlives one
  command execution.
- Deployment and rollback progress is rendered from semantic events, and a TUI
  rendering failure cannot alter deployment success, failure, or compensation.
- Focused adapter tests and the full required CI gates pass. Disposable-VM
  regression and environment-dependent checks are recorded as PASS, FAIL, or
  SKIP with their actual prerequisites.

## Blockers

None.

## Validation evidence

- Checkpoint 1: `cargo fmt --check`, clippy with `-D warnings`, all-feature
  tests, and the release build are green; markdown links and focused TUI
  invocation tests are green.
- Rootless Podman OCI tests: SKIP (3 ignored tests require a configured
  rootless Podman host). Disposable-VM regression is deferred to checkpoint 5.
