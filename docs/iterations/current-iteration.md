# Current Iteration

**Status:** completed

**Base:** `8328842` (`docs: record pre-v0.3 consolidation completion`)

**Approved design:** retired after iteration closure.

## Iteration - Bootstrap, VM, and E2E Hardening

Objective: make bootstrap and VM regression reproducible, idempotent, and able
to prove the operational guarantees required before reconciliation.

### Scope and Non-goals

- VPS bootstrap and VM provisioning now share host invariants; clean-VM
  acceptance and E2E become mandatory criteria.
- The iteration does not implement `pneuma reconcile`, a registry watcher,
  auto-deploy, API, TUI, OIDC, RBAC, multiple hosts, a new Linux user, or
  precompiled binary download.
- Execution follows the design checkpoints in the order below. The first
  unchecked item is the next authorized work.

## Checkpoints

- [x] Open the iteration and record the approved design.
- [x] Make preflight rerun-safe and pin the source with immutable `--ref`.
    Result: explicit `--ci-public-key`/`--ref` parsing rejects missing values,
    unknown options, branches, abbreviated SHAs, and invalid refs; `--ref`
    accepts a tag or full SHA and forces a detached checkout of the resolved
    commit on every rerun; the 80/443 listener accepts active Caddy on rerun and
    blocks any other owner. Gates are green and shell lint has no warnings.
- [x] Fix remote bootstrap acceptance-test assertions.
    Result: `remote_assert`/`remote_assert_rejected`/`ci_assert_*` capture
    stdout/stderr in logs, preserve the `ssh` exit status, and require remote
    success for content; clean Debian 13 acceptance by SHA `2bdc512` passed
    with 82 PASS/0 FAIL, and a forced remote assertion produced 1 FAIL and a
    nonzero exit; functional fixture/deployment remains in `dev-vm/test-all.sh`.
- [x] Share provisioning invariants between VPS and VM.
    Result: `scripts/lib/provision-host.sh` centralizes runtime invariants;
    static checks/ShellCheck and four gates are green, disposable VM provisioning
    was validated, and bootstrap acceptance by SHA reported 82 PASS/0 FAIL.
- [x] Enforce account and subordinate-ID invariants.
    Result: read-only preflight rejects insecure accounts and malformed,
    duplicate, overlapping, or insufficient ranges before mutations; safe
    alternative subids/subgids are preserved and absence selects the first free
    range of 65,536 IDs from 100000 without changing another user; linger is
    confirmed and directories recover modes on rerun. Fixture: 9 PASS/0 FAIL;
    ShellCheck and four Rust gates are green; a disposable Debian 13 clone
    validated fallback after a conflicting `dev` range and idempotence.
- [x] Make Caddy configuration atomic and idempotent.
    Result: a candidate in the destination filesystem is validated before the
    rename replacement, an identical rerun creates no backup or Caddy reload,
    and failure preserves the active file. Preflight accepts the Caddy listener
    only with a valid Caddyfile; ShellCheck and four Rust gates are green, and a
    disposable Debian 13 clone validated first installation, rerun, change
    backup, and an invalid candidate.
- [x] Prove bootstrap and rerun on a clean Debian 13 host.
    Result: a new `pneuma-dev-base` clone ran bootstrap and two reruns using
    immutable SHA `11b10111f59a6fea09524fc4bd78f1109e830cd3`, with 87 PASS/0
    FAIL. It validated a free range after `dev:100000:65536`, account, linger,
    directories, environment, Caddy, rootless Podman, binary, CI key, and doctor;
    the disposable clone was destroyed.
- [x] Add version-pinned shell lint to CI.
    Result: the `shell` job installs pinned ShellCheck 0.10.0 and shfmt 3.10.0
    and runs `bash -n`, ShellCheck, and shfmt on every tracked script; scripts
    were formatted and unused variables removed. Shell checks and four Rust
    gates are green.
- [x] Prove a failed candidate preserves the active release and real rollback.
    Result: E2E publishes an unhealthy candidate in the permitted `healthy-http`
    repository, requires `Deploy/Failed` deployment, Running runtime, and v1
    body; it then promotes v2 and invokes `pneuma deployment rollback healthy-http`,
    requiring v1 body and `Rollback/Succeeded` history. A disposable Debian 13
    clone passed the full cycle and was destroyed.
- [x] Prove reboot and recovery by boot ID.
    Result: E2E requires SSH to go down and return within the timeout and a new
    boot ID; it then confirms `user@<uid>.service`, Quadlet, container, `app
    status` Running, and v1 body. A disposable Debian 13 clone passed the cycle
    and was destroyed.
- [x] Make local HTTPS and CI SSH boundaries mandatory in E2E.
    Result: VM provisioning configures `local_certs`, host mapping, and a trusted
    CA; E2E requires public HTTPS and the internal transition of `redirect-public`.
    The suite requires the CI dispatcher for permitted `version`/deployment and
    rejects shell, PTY, forwarding, agent/X11, reading, and injection without
    changing history. A Debian 13 clone passed `test-all.sh` with 38 PASS/0
    FAIL/0 SKIP and was destroyed.
- [x] Prove semantic restore, synchronize docs, and run final regression.
    Result: the suite creates `e2e-before-backup`, generates a backup, creates
    `e2e-after-backup`, and confirms after restore that only the first exists;
    docs distinguish bootstrap clone, E2E clone, and production smoke. Final
    regression: clean bootstrap 87 PASS/0 FAIL using SHA
    `927eb8502285d4658c9455a3c69734bbf9ee95fd`; E2E 45 PASS/0 FAIL/0 SKIP.
    Both disposable clones were destroyed.

## Acceptance Criteria

- [x] Bootstrap accepts only full-SHA or tag `--ref` and reinstalls the resolved
  commit on every rerun.
- [x] A clean host and rerun validate user, subids, linger, directories,
  environment, Caddy, rootless Podman, binary, and CI key invariants.
- [x] VPS and VM call a shared implementation of host invariants.
- [x] Caddy is atomically updated and preflight accepts only managed Caddy on
  ports 80/443 during rerun.
- [x] CI runs `bash -n`, ShellCheck, and shfmt with pinned versions.
- [x] E2E proves a failed candidate preserves v1, real rollback, real reboot,
  public/internal HTTPS, CI key boundaries, and semantic restore.
- [x] Four gates and required bootstrap/E2E VM regressions are green, with no
  unaccepted skips.

## Blockers

None.

## Final Validation

At `eb1fce6`: `cargo fmt --check`, Clippy with `-D warnings`, `cargo test
--all-features`, and the release build are green; shfmt 3.10.0, `bash -n`, and
ShellCheck 0.10.0 are green. Clean bootstrap acceptance: 87 PASS/0 FAIL.
Disposable E2E: 45 PASS/0 FAIL/0 SKIP. The four ignored Rust tests require
rootless Podman configured on the local host; equivalent coverage ran on the
disposable VM.
