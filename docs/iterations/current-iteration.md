# Current Iteration

**Status:** in progress

**Base:** `5f270ed` (`docs(operations): keep dev-vm tutorial pointed only at repository files`)

**Approved design:** disposable regression automation plan approved by the user
in the working session of 2026-08-24; fixed decisions recorded below.

## Iteration - v0.4.3 Disposable Regression Automation

Objective: replace the three manual disposable-VM lifecycles of the final
regression with one orchestrator command, keeping every battery and its
semantics intact.

### Fixed decisions

- New `scripts/dev-vm/test-regression.sh` owns the full clone lifecycle:
  clone `pneuma-dev-base` as `pneuma-dev-base-test` through `qemu:///system`,
  pin a static DHCP lease so the IP survives in-suite reboots, boot and wait
  for SSH, provision, install the binary, install the CI key, run suites,
  and always destroy the clone (unless `--keep-on-fail` after a failure).
- Suites: `all` (default), `e2e`, `reconciliation`, `bootstrap`.
- `all` uses two clones: a shared clone runs `test-all.sh`, then a verified
  reset (`reset-fixtures.sh` plus an empty-application assertion plus registry
  restart), then `reconciliation-e2e.sh`; the bootstrap acceptance runs alone
  on its own pristine clone because it validates bootstrap from clean Debian.
- Root access resolution: provisioning SSH key first (`PNEUMA_VM_PROVISION_KEY`
  or `~/.ssh/pneuma-e2e-final`, generated under `~/.ssh/pneuma-provision` when
  absent); the root password is used only through `PNEUMA_VM_ROOT_PASSWORD`
  with `sshpass` when no key works, never written to repository files.
- A per-run isolated ssh-agent holds only the provisioning key so the existing
  battery scripts authenticate without user configuration changes.
- No changes to battery content, fixtures, reboot coverage, the base VM, CI
  gates, or parallel execution.

## Checkpoints

- [x] Orchestrator automates the disposable lifecycle (clone, DHCP pin, boot,
      access, provision, binary sync, CI key, suite dispatch, guaranteed
      destroy) with suite selection.
      Result: `scripts/dev-vm/test-regression.sh` with suites `all`, `e2e`,
      `reconciliation`, `bootstrap`; static DHCP lease survives in-suite
      reboots; run-local ssh-agent; password fallback only via
      `PNEUMA_VM_ROOT_PASSWORD`; EXIT trap destroys the clone on every path.
- [x] Shared-clone sequencing works end to end: `test-all.sh`, verified reset,
      registry restart, `reconciliation-e2e.sh`; bootstrap stays on its own
      pristine clone.
- [x] Fresh-clone battery defects fixed: the `pneuma database backup` path in
      `test-all.sh` is valid on a pristine VM.
      Result: backup path moved to a flat `/tmp` file that exists everywhere;
      the defect was invisible on long-lived dev VMs and only a fresh clone
      exposed it.
- [x] Tutorial section 6.5 documents the orchestrator as the standard path;
      AGENTS.md local note updated.

## Acceptance Criteria

- One `test-regression.sh all` run on this host finishes with both suites
  green (`0 FAIL / 0 SKIP` where batteries report counters), bootstrap green,
  and both clones destroyed including storage. **Met.**
- A failing or interrupted run leaves no orphan clone unless
  `--keep-on-fail` is set. **Met** (verified by three fast-fail attempts that
  each self-destroyed).
- `bash -n` and `shellcheck` pass for the new script; markdown link check
  passes; the four CI gates remain green. **Met.**

## Scope and Non-goals

- No product code, schema, or migration changes.
- Existing batteries are consumed as-is, except defects that only a fresh
  clone exposes (classified as necessary for the acceptance criterion); such
  fixes stay minimal.
- No parallel suite execution and no wall-clock optimization work.

## Blockers

- None.

## Validation Evidence

- Final green evidence on this host: `test-all.sh` 45/0/0 and
  `reconciliation-e2e.sh` 21/0/0 on the shared clone of a full `all` run;
  `test-bootstrap-vps.sh` 89/0 on its pristine clone via the dedicated
  `bootstrap` suite immediately after (first attempt hit a transient
  no-Internet flake in the VM; rerun on a fresh clone passed, exercising the
  suite-selector recovery path). Both clones destroyed including storage each
  time; static lease entries removed from the network config;
  `pneuma-dev-base` untouched throughout.
- Robustness hardening adopted after live incidents: every battery runs under
  `setsid` with detached stdin so terminal stop signals in the invoking shell
  cannot suspend probes (an interactive run froze twice on tty-stopped ssh
  probes before this); battery invocations use explicit `bash`; static DHCP
  entries are deleted at pin time and at destroy time.
- Offline gates on the final code state: `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, `cargo build --workspace --release`, markdown
  link check, `bash -n`, `shellcheck --severity=warning`.
- Defects found and fixed during live validation (each reproduced before the
  fix): silent SIGPIPE deaths from early-closing pipe consumers in lease
  discovery (`awk exit`, `head -1`); DHCP slot loop never assigning the
  candidate (assignment-prefix gotcha); static lease entries persisting across
  clone lifecycles; missing `restrict,command=` prefix when installing the CI
  dispatcher key — a real security gap only observable on a fresh clone;
  suite exit codes swallowed by function-return semantics; the `test-all.sh`
  database backup path absent on pristine VMs.
- Iteration duration evidence: full `all` run completes in roughly 35 minutes
  end to end on this host, single command, unattended.
