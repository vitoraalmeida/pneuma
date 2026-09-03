# CLI Operational Robustness

**Status:** approved on 2026-09-02

## Purpose

Correct all CLI robustness, error-classification, presentation, bootstrap, and
test-organization issues found in the post-v0.5.3 review. Presentation
failures can currently panic the deployment path, stable CLI text depends on
domain `Debug` formatting, doctor rendering drops captured diagnostics and can
panic, lock contention is indistinguishable from lock infrastructure failure,
nested deployment failures are classified coarsely, the host environment file
is silently ignored when malformed, and the binary CLI regression target is a
single unmanageable file.

## Approved Behavior Changes

The following observable behavior changes are approved and exhaustively
describe the intended exit-code and wording corrections:

| Scenario | Current | Corrected |
|---|---|---|
| stderr progress write fails | Deployment may panic | Deployment continues |
| systemd/Podman/Caddy/health deployment failure | Exit 1 | Exit 5 |
| application lock open/acquire failure | Exit 4 | Exit 1 |
| actual application contention | Exit 4 | Exit 4 (unchanged) |
| nested missing application/release | Exit 1 | Exit 3 |
| missing delivery/source configuration | Exit 1 | Exit 4 |
| missing default branch | Exit 1 | Exit 2 |
| persisted invalid visibility | Exit 2 | Exit 1 |
| import missing required system | Exit 1 | Exit 2 |
| malformed/unreadable host file | Silently ignored | Exit 1 with diagnostic |
| failed Git/Podman/Caddy doctor command | Generic `command failed` | Include captured detail |
| `ActiveOciImages(Passed)` | Panic | Render successful result |

## Fixed Decisions

1. Perform an exhaustive semantic error-classification audit; the corrected
   table above is the authoritative target for every scenario it lists.
2. A missing host environment file remains optional startup behavior.
3. Unreadable, malformed, duplicate, invalid UTF-8, or invalid-variable host
   environment files fail startup with one contextual `error:` line, exit 1,
   empty stdout, no database creation, no external command, and before
   argument parsing.
4. Caller-supplied environment values continue to override file values.
5. A nonempty `HOME` or `PNEUMA_QUADLET_DIR` is required after environment
   derivation.
6. Progress output is best effort: a failed stderr write can never interrupt
   deployment execution, without `catch_unwind` and without changing observer
   signatures, spinner frames, timing, event ordering, TTY selection, or final
   output.
7. Successful stdout/stderr bytes remain unchanged unless listed in the
   corrected table above.
8. Doctor command failures expose captured diagnostic detail as
   `command failed (<detail>)`, keeping the generic line when detail is
   exactly `command failed`; publicly constructible doctor results render
   totally without `unreachable!`.
9. Domain enum `Debug` formatting is removed from stable CLI text without
   changing existing bytes; presentation labels are owned explicitly by the
   CLI, including the `Unknown { status: "..." }` representation.
10. Application lock `Open`/`Acquire` failures classify as `Failure` (exit 1);
    `ApplicationBusy` remains `Conflict` (exit 4). Nested deployment failures
    classify by typed semantic cause with no string matching or dynamic
    downcasting.
11. CLI integration tests receive a full staged module split with one shared
    support module and per-capability scenario modules, one Cargo target.
12. Host environment validation stays in initial single-threaded startup with
    all unsafe environment mutation, reading `PNEUMA_HOST_ENVIRONMENT_FILE`
    (default `/etc/pneuma/environment`).

## Non-Goals

- Do not add dependencies, async runtimes, generic event frameworks, observer
  traits, output modes, terminal abstractions, migrations, or v0.6 features.
- Do not change domain `Display` implementations or public diagnostic enums.
- Do not change numeric exit-class definitions (1-5) or error wording beyond
  the corrected table.
- Do not change CLI syntax, Clap grammar, or CI SSH grammar.
- Do not change persistence, external adapters, or deployment logic except
  where a corrected table row requires classification at the CLI edge.
- Do not mark the roadmap v0.5.4 release complete or begin v0.6 as part of
  this iteration.

## Acceptance Scenarios

- A non-TTY stderr that rejects writes cannot abort or fail a deployment.
- TTY progress emits the established lifecycle text, spinner frames, and
  clear-line control bytes; non-TTY stable text and verbose ordering remain
  byte-for-byte unchanged.
- Every scenario in the corrected table behaves exactly as corrected; all
  other output remains byte-for-byte unchanged.
- A malformed or unreadable host environment file fails startup atomically
  with a diagnostic; a missing file continues to boot.
- Doctor output preserves captured failure detail and renders every
  publicly constructible outcome without panic.
- The CLI integration suite is organized into capability modules with shared
  support and no duplicated harness or fake implementations.
- Each code checkpoint passes `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`; the
  closure checkpoint additionally passes markdown-link validation, the
  disposable-VM E2E battery, and records environment-dependent checks
  honestly (PASS/FAIL/SKIP with reasons).

## Checkpoint Order

1. Establish governance, the v0.5.4 tracker, and the behavioral baseline at
   `6227c0e`.
2. Make progress output best effort (observer and progress isolation).
3. Make presentation labels explicit (remove stable CLI dependence on domain
   `Debug`).
4. Preserve doctor diagnostics during rendering (total doctor rendering).
5. Distinguish lock failures from contention.
6. Classify nested deployment failures semantically.
7. Complete the remaining semantic error classification audit.
8. Validate the host environment before startup.
9. Cover adapter-only invocation paths.
10. Extract shared CLI integration test support.
11. Isolate reconciliation test scenarios.
12. Isolate lifecycle test scenarios.
13. Isolate exposure test scenarios.
14. Isolate deployment test scenarios.
15. Complete catalog and database test modules.
16. Run operational regression and close the iteration.

## Baseline And Ownership Notes

- The observer seam refactor (named `ignore_events`, `observer` parameter
  names, corrected `execute` comment) was committed as `6227c0e` ("Change
  events configuration") outside the planned checkpoint flow; its content is
  verified identical to the reviewed draft. It is retained as-is; checkpoint 2
  above covers only the CLI-side isolation work.
- The governance baseline was established on `6227c0e` with a clean worktree.
