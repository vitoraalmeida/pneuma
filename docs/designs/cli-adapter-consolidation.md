# CLI Adapter Consolidation

**Status:** approved on 2026-09-01

## Purpose

Consolidate the CLI adapter after v0.5.1 moved command execution into the
interface-neutral control boundary. The current CLI has a second command
vocabulary in `src/cli/args.rs` and separate command-family handlers that repeat
argument-to-control-command mapping, executor invocation, result extraction,
error mapping, and output dispatch. This iteration removes that duplication
without changing the public CLI contract or the control boundary's ownership.

## Fixed Decisions

1. Clap remains private to `src/cli/args.rs`. Parsed CLI input maps directly to
   `pneuma::control::Command` wherever the invocation reaches control; no
   parallel CLI command enum mirrors the control vocabulary.
2. `version` remains a CLI-only, lock-free operation. Restricted CI dispatch
   remains a CLI adapter concern: it reads `SSH_ORIGINAL_COMMAND`, validates its
   existing grammar, and maps its permitted deployment request to the same
   control command as the interactive CLI.
3. One CLI dispatch path owns execution of ordinary control commands, conversion
   from `ControlError` to `CliError`, and result rendering. Deployment commands
   may use the existing event-capable execution path solely to attach the
   CLI-only progress renderer.
4. One result-rendering match owns the correspondence between every
   `CommandResult` variant and the existing output functions. It may retain
   small private helpers only where a result has presentation-specific policy,
   such as doctor health failure or empty list output.
5. `src/cli/output.rs` remains the owner of text formatting. It continues to
   receive typed control results and use-case read models; no rendered strings,
   terminal concerns, or exit-code policy move into `src/control/`.
6. The CLI retains its current public syntax, stdout, stderr, verbose lines,
   non-TTY output, TTY animation, error wording, and exit-code classes. The
   change is structural only.
7. No domain, use-case, adapter, persistence, migration, host-configuration,
   or external-effect behavior changes. No production dependency, async
   runtime, generic command framework, trait hierarchy, or compatibility shim
   is introduced.

## Non-Goals

- No new command, option, output mode, machine-readable output, color policy,
  TUI, HTTP adapter, daemon, or transport protocol.
- No change to the restricted CI command grammar or SSH trust boundary.
- No change to semantic deployment events or the concurrent renderer's TTY
  behavior.
- No redesign of `ControlExecutor`, `Command`, `CommandResult`, or
  `ControlError` for hypothetical future adapters.
- No behavior change in output whitespace, diagnostics, errors, or exit codes.

## Acceptance Scenarios

- Every non-version interactive command maps from Clap input to exactly one
  control command without an intermediate CLI command enum that duplicates it.
- Every `CommandResult` variant has one exhaustive CLI rendering path, and an
  unexpected command/result pairing is impossible or rejected by a local,
  explicit invariant.
- Image deploy, branch deploy, rollback, and CI branch deploy all share the
  event-capable CLI execution path and preserve their current progress output.
- `doctor` still renders its report when checks fail or the database cannot be
  opened, then returns the same existing `CliError` class.
- CLI integration tests prove unchanged syntax, stdout, non-TTY stderr, and
  exit codes for each command family. Focused unit tests cover parsed-command
  mapping and exhaustive rendering policy.
- The control integration tests continue to execute command families directly,
  without Clap or terminal output.
- The required Rust and documentation CI checks pass. Environment-dependent OCI
  tests and disposable-host regression are recorded accurately rather than
  claimed when unavailable.

## Checkpoint Order

1. Establish governance and a CLI output/exit-code baseline from v0.5.1.
2. Map parsed interactive arguments directly to control commands and remove the
   duplicate CLI command vocabulary while retaining version and CI adapter-only
   dispatch.
3. Consolidate ordinary control execution and exhaustive result rendering;
   remove redundant command-family handler modules without changing text or
   error classification.
4. Consolidate deployment event execution and restricted CI deployment routing;
   prove interactive and CI output contracts with focused CLI regressions.
5. Synchronize implemented documentation and run the full required regression
   ladder before closing the iteration.

Governance and baseline recording precede checkpoint 2. Each implementation
checkpoint is independently green, updates the active tracker, and does not
begin the next checkpoint early.
