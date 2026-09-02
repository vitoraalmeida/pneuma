# Consolidated Pneuma Roadmap - v0.1 to v1.0

**Status:** living document - project evolution contract
**Pilot application:** `vitoralmeida.tech`

This document records project evolution and future direction. It is not the
source of truth for current implemented architecture. See
[`architecture/architecture.md`](architecture/architecture.md) for current
behavior, [`architecture/data-model.md`](architecture/data-model.md) for current
persistence, [`../CHANGELOG.md`](../CHANGELOG.md) for released changes, and
[`iterations/current-iteration.md`](iterations/current-iteration.md) for active
implementation.

Completed-version delivery detail remains in Git history. The summaries below
deliberately do not redefine current architecture.

## Principles

1. **Core does not know interfaces** - command interfaces call the same use cases.
2. **Idempotence** - repeating an operation does not duplicate a resource.
3. **Desired state is not observed state** - SQLite records intent; external
   systems report reality.
4. **Application survives Pneuma** - the host supervises runtime.
5. **Immutable Release** - an OCI digest identifies the artifact; a Deployment is
   an activation attempt.
6. **Independent exposure** - visibility and runtime are orthogonal.
7. **Pneuma is operable** - it is installable, updatable, diagnosable, and
   recoverable.

## Completed Versions

### v0.1 - OCI Foundation

**Status:** completed on August 8, 2026.

Established the single-host OCI foundation: Application and System catalog,
digest-pinned Release deployment, candidate health validation, rootless Quadlet
runtime, Caddy exposure, rollback history, diagnostics, and backup/restore.
Local-build delivery introduced during this stage was removed in v0.2.

### v0.2 - Git-Aware OCI Delivery

**Status:** completed on August 10, 2026.

Established `Git -> CI -> OCI -> digest-pinned Release -> Deployment` as the
normal delivery path. Added remote-Git import, manifest schema v3, branch/tag
resolution, full commit-SHA image tag resolution, capability-oriented
persistence, and the restricted SSH CI dispatcher.

### v0.3 - Consolidation and Operational Hardening

**Status:** completed on August 13, 2026.

- Deployment and RuntimeInstance became first-class domain types.
- Release became artifact-only; source provenance belongs to Deployment.
- Bootstrap, rootless Podman account invariants, Caddy replacement, and CI key
  provisioning became reproducible and validated.
- Disposable Debian 13 E2E proved candidate-failure preservation, rollback,
  reboot recovery, local HTTPS, CI SSH boundaries, and semantic SQLite restore.

### v0.3.1 - Caddy Routing Fix

**Status:** completed on August 13, 2026.

- Unmatched HTTP hosts receive generic `404 Not Found`.
- Internal visibility removes the public route while preserving the running
  loopback runtime.
- Bootstrap reruns correctly identify active Caddy listeners on ports 80/443.

## v0.4 - Reconciliation and Deployment Reliability

**Status:** completed on August 21, 2026.

Pneuma converged runtime and exposure materialization toward persisted intent
without selecting a new Release or creating a Deployment from drift.

- [x] Desired versus observed state.
- [x] Drift detection and unambiguous recovery.
- [x] Interrupted Deployment recovery.
- [x] Better restart/reboot convergence.
- [x] Candidate and activation improvements.
- [x] Deployment mutual exclusion per Application.

Deferred from v0.4, still open for a future version:

- Non-interactive CLI with structured output and exit codes.

Already implemented, not v0.4 work: CI validation and image publishing under the
full commit-SHA tag; restricted SSH deployment through `pneuma ci dispatch`.

Out of scope until demonstrated need: registry watchers, automatic deployment
policies, complete audit logging, generic idempotency keys, image retention, and
automatic rollback after promotion.

### v0.4.1 - Domain Hardening Sweep

**Status:** completed on August 21, 2026 (iteration stage; not tagged separately
- its changes shipped within the v0.4.2 release).

- Loopback runtime endpoints became a domain-owned invariant
  (`ExpectedRuntimeEndpoint`), fixing IPv6 `::1` acceptance in internal health
  checks, with `ContainerPort` typed through runtime registration.
- Visibility changes probe the persisted health check path and status instead of
  a hardcoded `/` expecting `200`.
- The Deployment transition table, promotion eligibility, exposure target and
  outcome types, stable runtime naming, and container-id format rules each have
  one domain owner.
- Caddy, lock, port, OCI, and reconciliation boundaries accept
  typed identities; `ApplicationName`/`SystemName` share one catalog-name
  validator.

### v0.4.2 - Domain Type Closure

**Status:** completed on 2026-08-21.

- Runtime lifecycle use cases and candidate cleanup accept `ApplicationName`.
- `OciArtifact` flows through OCI deploy, pull, and rollback as a typed value;
  parsing happens once at the CLI edge or branch-resolution boundary.
- Container observation, container-id resolution, external health checks, and
  Quadlet unit rendering accept typed domain identities; duplicated validation
  removed.
- `HostPort` newtype represents the reserved loopback host port.

### v0.4.3 - Disposable Regression Automation

**Status:** completed on 2026-08-27 (tagged and released).

- `scripts/dev-vm/test-regression.sh` automates the full disposable-VM
  regression in one command: clone, provision, suite dispatch (`all`, `e2e`,
  `reconciliation`, `bootstrap`), and guaranteed clone destruction.
- Rollback and reconciliation battery defects fixed on fresh clones: the
  rollback happy path persists a hydratable runtime tombstone, and
  reconciliation classifies unknown external states conservatively.
- Error and type redundancy cleanup across the deployment pipeline: one
  canonical `FailedExecution` failure carrier, one durable failure-code
  vocabulary in the domain, and table-driven CLI classification.

### v0.5 - Architecture Simplification

**Status:** completed on 2026-08-31 (tagged and released).

The obsolete compatibility, ownership, and migration machinery was replaced by
the current smaller architecture. This release intentionally contains an
incompatible database reset: existing v0.4 databases are rejected, not
upgraded.

- One baseline schema (`migrations/0001_current_schema.sql`) replaces the
  historical migration chain; the runner initializes empty databases
  atomically and rejects every other schema.
- One per-Application kernel lock guards every existing-Application mutation;
  operation tokens, generations, and ownership storage were removed, and
  contention is caller-visible (`Deferred` for reconciliation).
- Strict domain types: required `SystemId`, validated 32-hex entity IDs,
  optional validated `CommitSha` source revisions, and complete typed failure
  evidence.
- A database-wide lock serializes normal access against restore; restore
  accepts only exact-current, integrity-checked backups and keeps a
  pre-restore snapshot.
- Retirement records success only after observing container absence.

### v0.5.1 - Interface-Neutral Execution

**Status:** completed on 2026-08-31.

Command execution moved out of the CLI into a synchronous, interface-neutral
control boundary. The CLI is one adapter among possible future interfaces; no
daemon, HTTP server, or TUI was added.

- [x] Control boundary for every stateful command, migrated one command family
  at a time with unchanged CLI behavior.
- [x] Semantic deployment events with matched start/completion boundaries,
  typed failure codes, and typed retirement warnings.
- [x] Animated TTY progress rendered entirely in the CLI, with deterministic
  non-TTY output.

### v0.5.2 - CLI Adapter Consolidation

**Status:** completed on 2026-09-01. Approved design:
[`designs/cli-adapter-consolidation.md`](designs/cli-adapter-consolidation.md).

The CLI adapter removed its duplicated command vocabulary and repetitive
per-command execution handlers, retaining all established command syntax,
output, progress, error, and exit-code behavior.

- [x] Approved design; activate the execution tracker after the design commit.
- [x] Map parsed interactive input directly to control commands.
- [x] Consolidate control execution, result rendering, deployment progress, and
  restricted CI routing.

### v0.5.3 - CLI Adapter Integrity

**Status:** completed on 2026-09-02. Approved design:
[`designs/cli-adapter-integrity.md`](designs/cli-adapter-integrity.md).

This maintenance iteration corrected internal CLI adapter imprecision without
changing the released CLI or operational contract.

- [x] Reject missing interactive deploy sources during argument normalization.
- [x] Consolidate deployment classification and remaining lifecycle rendering
  ownership within the CLI adapter.

### v0.5.4 - CLI Operational Robustness

**Status:** in progress. Approved design:
[`designs/cli-operational-robustness.md`](designs/cli-operational-robustness.md).

This maintenance iteration corrects all CLI robustness, error-classification,
presentation, bootstrap, and test-organization issues found in the post-v0.5.3
review. Approved observable corrections are enumerated in the design's
behavior-change table.

- [ ] Make progress output best effort and presentation labels explicit.
- [ ] Preserve doctor diagnostics and total rendering.
- [ ] Complete the semantic error-classification audit (locks, nested
  deployments, remaining scenarios).
- [ ] Validate the host environment contract before startup.
- [ ] Reorganize CLI integration tests into capability modules.

## v0.6 - Observed State / Host Observation

**Status:** planned; not started. No approved design yet. Queued behind
v0.5.4.

Objective: stop depending predominantly on the state Pneuma itself recorded and
start explicitly observing the real state of the host. This version establishes
the separation between:

- **Desired State** - what should exist;
- **Recorded State** - what Pneuma believes it did;
- **Observed State** - what actually exists right now.

- [ ] Workload observation: query systemd/Podman for unit existence and unit
  state (`active`, `inactive`, `failed`), container existence, container running
  state, current PID, the image/digest actually in use, and exit status where
  applicable.
- [ ] Proxy observation: verify what Caddy actually publishes - whether the
  application has a route, whether the expected domain points at the correct
  target, whether routes that should be absent really are absent, and whether
  public exposure diverged from desired state.
- [ ] Explicit observed-state model: a desired application state compared with an
  observed counterpart produces a small verdict set (conceptually
  `InSync`/`Missing`/`Unexpected`/`Different`/`Unknown`; naming is indicative,
  not literal).
- [ ] Observation-based reconciliation: the reconciler stops asking only "did my
  previous operation finish?" and starts asking "is the world really in the
  expected state?".
- [ ] Unknown-state handling: not every observation must collapse into
  healthy/failed. `Unknown`, `Unobservable`, and partially observed results are
  legitimate outcomes so Pneuma never invents certainty.

Result: after v0.6, manually killing a container or editing a unit must let
Pneuma report "the desired state is X, but I observed Y" before attempting any
repair.

## v0.7 - Recovery & Resilience

**Status:** planned; depends on v0.6 observed state.

Objective: turn reconciliation from "try to reach the expected state" into a
mechanism that stays robust when something breaks during reconciliation itself.
v0.6 answers "is the system divergent?"; v0.7 answers "can I recover this system
safely?".

- [ ] Idempotent operations: repeating an operation must not corrupt state.
  Prefer `ensure_*` operations (`ensure_container_running`,
  `ensure_route_exists`, `ensure_unit_installed`, `ensure_release_active`) over
  imperative `create_*/add_route/start_*` where semantically meaningful.
- [ ] Crash recovery: if Pneuma dies mid-deployment (candidate created, process
  gone), a fresh start must observe the host and decide what to do.
- [ ] Partial failure coverage: combinations such as "container created, unit
  installed, proxy not updated" or "new release started, proxy switched, old
  release not removed".
- [ ] Retry policy: distinguish transient errors, permanent errors, unknown
  state, safe-to-retry operations, and operations that require inspection.
- [ ] Recovery of incomplete operations: operation/deployment progress must be
  reconstructable from intent + persisted state + observed state, not from a
  single running flag.
- [ ] Fail-safe behavior: when Pneuma cannot know what happened, prefer the safe
  outcome - never destroy the previous release without sufficient evidence that
  the new one is healthy.
- [ ] Chaos testing: kill Pneuma mid-deploy, kill containers, block systemd
  restarts, force invalid Caddy reloads, remove Quadlet units, simulate timeouts,
  and repeat reconciliations many times.

Result: intermediate events stop being the source of truth; current state
becomes reconstructable.

## v0.8 - Multi-Service Applications

**Status:** planned.

Objective: move from one workload per Application to Applications composed of
multiple services (for example `gateway`, `api`, `auth`, `worker`). Pneuma starts
managing systems of cooperating services rather than isolated containers.

- [ ] Service as an explicit concept: an Application owns a list of services,
  each with its own image, command, environment, health check, resources,
  exposure, and dependencies.
- [ ] Composite Release: a Release represents one consistent configuration of
  several services (per-service digests under one release identity). Rollback
  restores the whole prior Release, never one service.
- [ ] Initial internal networking: services of the same application can find
  each other through a simple runtime-provided mechanism (for example
  `auth.internal`/`api.internal` or equivalent).
- [ ] Dependency ordering: simple startup ordering where there is real need
  (database ready → auth → api) without building a homegrown scheduler.
- [ ] Aggregate health: application-level health derived from service states
  (conceptually Healthy/Degraded/Unhealthy).
- [ ] Per-service reconciliation: an application can be divergent in one service
  while others are in sync; act on the smallest appropriate unit.

## v0.9 - Workload Identity / mTLS

**Status:** planned.

Objective: give workloads a verifiable cryptographic identity so services can
authenticate each other. v0.8 creates real relationships between services; v0.9
secures them.

- [ ] Workload identity per service (conceptually
  `spiffe://pneuma.local/application/shop/service/auth`).
- [ ] SPIFFE/SPIRE or an equivalent mechanism - do not invent a private PKI if
  SPIFFE solves the problem; the host may run a SPIRE agent or equivalent.
- [ ] Short-lived credentials: renewable, identity-bound, never distributed as
  long-lived static secrets.
- [ ] mTLS between services (frontend → api, api → auth).
- [ ] Identity-aware policy foundation: enough to express "auth accepts
  application/shop/service/api" without becoming a full authorization system.

Result: a process is no longer trusted merely because it runs on the same host;
it must prove which workload it is.

## v0.10 - Resource Isolation

**Status:** planned.

Objective: prevent one application from harming the host or other applications
through excessive resource consumption, using Linux/systemd primitives rather
than an internal scheduler.

- [ ] Memory limits per service with predictable overrun behavior.
- [ ] CPU limits or weights depending on the adopted model.
- [ ] PID/task limits to contain fork bombs and process leaks.
- [ ] Filesystem constraints where meaningful: read-only root filesystem,
  explicit volumes, write limits, `tmpfs`, path separation.
- [ ] Enforcement stays in systemd/cgroups.
- [ ] Resource observation: memory usage, CPU usage, PID counts, and OOM events -
  not only configuration.
- [ ] Resource-policy reconciliation: manual drift such as changing
  `MemoryMax=512M` to `MemoryMax=infinity` must be detectable.

## v0.11 - Network Isolation

**Status:** planned.

Objective: move networking from "everything on the host can potentially talk"
to "communication must be explicitly allowed".

- [ ] Per-application networks by default: two applications cannot see each other.
- [ ] Explicit connectivity policy between services (allow/deny specific edges).
- [ ] Default deny between applications unless explicitly allowed.
- [ ] Explicit ingress: Internet → Caddy → allowed service only; internal
  services stay unexposed.
- [ ] Simple egress classes initially (allow internet / deny internet / allow
  destinations) without an elaborate firewall DSL.
- [ ] Integration with v0.9 identity: networking decides whether traffic may
  reach a target, identity decides who is trying, mTLS proves it.

Result: a compromised application loses automatic lateral movement on the host.

## v1.0 - Hardening

**Status:** planned.

v1.0 introduces no major new abstraction. It takes everything that exists and
answers: would I trust this to keep my applications running for months without
manual intervention?

- [ ] Documented failure model with defined behavior for each case: host reboot,
  disk full, process crash, container crash, corrupted state, invalid manifest,
  Caddy failure, systemd failure, network unavailable, interrupted operation.
- [ ] Security review covering the SSH boundary and dispatcher forced command,
  filesystem permissions, Podman privileges, systemd units, secrets,
  SPIFFE/SPIRE integration, network policy, and manifest validation.
- [ ] Manifest/versioning stability: define manifest compatibility (for example
  `apiVersion: pneuma.dev/v1`).
- [ ] Robust database migrations: tested upgrades, backup, detectable
  corruption, rollback where possible.
- [ ] Supported upgrade path for Pneuma itself - updating must never mean
  "hope it keeps working".
- [ ] Operational observability: enough information to answer what happened,
  when, which reconciliation ran, what was observed, and why an action executed -
  without becoming Grafana inside Pneuma.
- [ ] Extensive failure testing: happy path, drift, crash, reboot, partial
  deployment, bad configuration, network failure, resource exhaustion.
- [ ] Documentation explaining invariants, consistency model, failure model,
  reconciliation model, deployment model, identity model, and trust boundaries.

## Progression

Each version creates the conceptual prerequisite of the next:

```text
v0.4.2  basic reconciliation
   │
   ▼
v0.4.3  disposable regression automation
   │
   ▼
v0.5    Architecture Simplification "a smaller current architecture"
   │
   ▼
v0.5.1  Interface-Neutral Execution "one boundary, many interfaces"
   │
   ▼
v0.5.2  CLI Adapter Consolidation   "one adapter path"
   │
   ▼
v0.5.3  CLI Adapter Integrity       "precise adapter boundaries"
    │
    ▼
v0.5.4  CLI Operational Robustness  "a CLI that fails honestly"
    │
    ▼
v0.6    Observed State            "what is really happening?"
   │
   ▼
v0.7    Recovery & Resilience     "can I return to a valid state?"
   │
   ▼
v0.8    Multi-Service             "can I administer a system?"
   │
   ▼
v0.9    Identity / mTLS           "who is each workload?"
   │
   ▼
v0.10    Resource Isolation        "how much may each workload consume?"
   │
   ▼
v0.11   Network Isolation         "who may talk to whom?"
   │
   ▼
v1.0    Hardening                 "can I trust this operationally?"
```

## Not Scheduled

Deferred until demonstrated need: artifact security (SBOM generation and
enforcement, image signature verification, admission policies for unsigned
artifacts), secret management/injection/rotation, non-interactive CLI with
structured output and exit codes.

Beyond v1.0, still out of scope: HTTP API, webhooks, centralized observability,
multiple hosts, scheduler, remote agents, distributed reconciliation, managed
builds, canary or gradual rollout, autoscaling, Kubernetes, RBAC, and multi-user
support remain out of scope until a future version explicitly revisits them.
