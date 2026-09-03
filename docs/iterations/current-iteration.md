# Current Iteration

**Status:** em andamento

**Base:** `19447d4` (`Update ci`)

**Approved design:** none. Single maintenance checkpoint authorized directly by
the repository owner on 2026-09-02; no product behavior changes.

## Iteration - VM E2E post-reboot readiness (maintenance)

Objective: remove the boot-timing race in the E2E battery's post-reboot
verification without weakening the automatic-recovery requirement. The PR
E2E run of `19447d4` failed at Step 9 of `scripts/dev-vm/e2e.sh` about seven
seconds after the guest returned from reboot, while the immediately prior
full E2E run of the same product code passed; SSH recovers before Quadlet
and Podman finish regenerating and starting the application unit.

## Checkpoints

1. [x] Bounded post-reboot readiness in `scripts/dev-vm/e2e.sh`
   - Wait boundedly for the pneuma user manager, the healthy-http Quadlet
     service, and its container after the guest reboot, polling instead of
     asserting once; on timeout, print unit, Quadlet source, user-journal,
     and Podman diagnostics before failing.
   - The automatic-boot assertion stays intact: `pneuma reconcile` only runs
     after the generated service and container are confirmed active, so the
     step still proves Quadlet boot recovery and not reconciliation repair.
   - Result: Step 9 now polls each readiness boundary (user manager 60s,
     Quadlet service 120s, container 60s) and reports the listed diagnostics
     before failing; the immediate post-SSH assertions were the only change,
     and the reconciliation/status/HTTP checks after the waits are unchanged.

## Acceptance criteria

- Step 9 passes on a slow or loaded guest and fails with actionable
  diagnostics on a genuinely broken boot.
- `bash -n`, ShellCheck 0.10.0, and shfmt 3.10.0 pass over all tracked shell
  scripts (the CI `shell` job gates, with the same pinned versions).
- The full disposable-VM regression (`scripts/vm/run-e2e.sh` with the
  reconciliation drift catalog, matching the PR E2E job) passes locally.

## Blockers

None.

## Validation evidence

- Shell gates: `bash -n`, ShellCheck 0.10.0, shfmt 3.10.0 over all tracked
  scripts — clean.
- Full local disposable-VM regression: `scripts/vm/run-e2e.sh` with
  `PNEUMA_VM_RECONCILIATION=1` on the KVM host — battery passed including
  the reboot Step 9 wait path, and the reconciliation drift catalog passed
  21/21 including the R6/R7 reboot cases.
- Rust CI gates re-run for the checkpoint: `cargo fmt --check`, clippy
  `-D warnings`, `cargo test --all-features`, release build — green.
