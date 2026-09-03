# CLI Adapter Integrity

**Status:** approved on 2026-09-01

## Purpose

Correct remaining internal CLI adapter imprecision after v0.5.2 without changing
the public CLI or operational behavior. Invalid `app deploy` input currently
survives argument normalization as an executable invocation target, deployment
classification is duplicated in execution dispatch, and three lifecycle result
headings are assembled outside the output module.

## Fixed Decisions

1. `app deploy <application>` without `--image` or `--branch` fails during
   argument normalization as `CliError::MissingDeployOption`. It never reaches
   CLI dispatch, control, SQLite, or external commands. Clap continues to reject
   simultaneous image and branch options using its current grammar and text.
   Argument tests exercise that grammar through `Cli::try_parse_from`, including
   global `--verbose`, image and branch sources, the missing-source error, and
   conflicting sources.
2. `InvocationTarget` contains only `Control(Command)`, `Version`, and
   `CiDispatch`. `TryFrom<Commands>` performs the fallible normalization.
3. `app deploy` remains the interactive deployment syntax and `deployment
   rollback` remains the top-level deployment-group command.
4. `ControlExecutor`, `control::Command`, `CommandResult`, and `ControlError`
   remain unchanged. This iteration changes no domain, use case, adapter,
   database, configuration, or external-effect behavior.
5. One private CLI deployment classifier determines whether a control command
   uses event-capable execution. CI continues to validate its restricted grammar
   then route branch deployment to the existing shared control command path.
6. `src/cli/output.rs` owns all final application-status and lifecycle text.
   `src/cli/mod.rs` retains rendering policy such as empty-list suppression and
   doctor failure handling. `application_list` already owns its final whitespace,
   so dispatch does not trim or copy its rendered result.
7. CLI names, options, help, stdout, stderr, whitespace, verbose lines, exit
   codes, non-TTY progress, TTY animation, and CI SSH grammar remain unchanged.

## Non-Goals

- Do not move `app deploy` to `deployment deploy`.
- Do not merge image and branch deployment control commands.
- Do not alter Clap grammar to remove the custom missing-source error.
- Do not add a control error for missing deploy options.
- Do not add dependencies, traits, generic command frameworks, async runtimes,
  terminal abstractions, output modes, or compatibility code.
- Do not change persistence, migrations, external adapters, or deployment logic.

## Acceptance Scenarios

- Missing interactive deploy source returns the established CLI error and exit
  class without creating a database or invoking an external command.
- Image and branch deployment preserve their direct mappings; conflicting source
  options preserve Clap rejection.
- Image deploy, branch deploy, rollback, and CI branch deploy retain their
  existing event and progress sequences through one classification decision.
- Application status, start, and stop output remain byte-for-byte unchanged.
- Focused argument, rendering, and binary CLI regressions prove the preserved
  contract; required Rust CI gates pass for each code checkpoint.

## Checkpoint Order

1. Establish governance and the v0.5.2 behavioral baseline.
2. Make argument normalization fallible and reject a missing deploy source
   before dispatch.
3. Give deployment execution classification one CLI owner.
4. Move remaining lifecycle result text to the output module.
5. Run operational regression and close the iteration.
