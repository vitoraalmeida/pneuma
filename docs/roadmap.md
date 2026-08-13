# Consolidated Pneuma Roadmap — v0.1 to v0.7

**Status:** living document — project evolution contract
**Pilot application:** `vitoralmeida.tech`

## Unified Flow (v0.1 → v0.2)

```text
Git branch
    ↓ (v0.2: app deploy --branch)
git ls-remote → CommitSha
    ↓ (v0.2: `image:<commit>` convention)
OCI Registry (ghcr.io)
    │   image@sha256:...
    ▼
Create Release
    │
    ▼
DeployRelease
```

CI produces the artifact (such as `image:<commit-sha>`), and Pneuma discovers
and operates the commit artifact. v0.1 still accepts a Release from a local
build (`deploy-source`); v0.2 removes that path and makes `Git → CI → OCI → Release
→ deployment` the only flow.

## Principles

1. **Core does not know interfaces** — CLI, TUI, and API call the same use cases.
2. **Idempotence** — repeating an operation does not duplicate a resource.
3. **Desired state ≠ observed state** — desired in SQLite, observed in Podman.
4. **Application survives Pneuma** — runtime supervised by the host.
5. **Immutable Release** — identified by OCI digest; deployment is an attempt.
6. **Independent exposure** — visibility and runtime are orthogonal.
7. **Pneuma is operable** — installable, updatable, diagnosable, recoverable.

---

## v0.1 — OCI Foundation

Pneuma registers applications and deploys immutable OCI releases with basic
operational safety, preserving the healthy version when a new deployment fails.

### Target Entities

```text
System (new)
└── Application
      ├── desired_runtime_state
      ├── desired_visibility
      ├── active_deployment_id
      ├── delivery_spec (type, image_repository)
      │
      └── Release (new)
            └── Deployment
                  └── RuntimeInstance
```

### Already Implemented

| Capability | Status | Notes |
|---|---|---|
| Application entity + catalog | ✅ | |
| SQLite + migrations (13) | ✅ | |
| Deployment persistence + state machine | ✅ | |
| RuntimeInstance persistence | ✅ | |
| Podman rootless (create, start, stop, inspect) | ✅ | Local build removed in v0.2 |
| Start / stop / status | ✅ | |
| Internal health check | ✅ | |
| External health check | ✅ | |
| Safe traffic switch (failure preserves healthy runtime) | ✅ | |
| Caddy integration + public routing | ✅ | |
| Exposure materialization state | ✅ | |
| Deployment history | ✅ | |
| CLI (import, list, status, deploy, start, stop, visibility set, deployments, version) | ✅ | |
| Rollback (new deployment of the prior Release, does not depend on the old container) | ✅ | |
| Visibility set (public/internal) independent from lifecycle | ✅ | |
| Doctor (13 checks: DB, migrations, workspace, Caddy dirs, Caddyfile/config, git, podman, rootless, Quadlet generator, OCI images, disk, caddy) | ✅ | |
| Version | ✅ | |
| Staging validation (`staging.vitoralmeida.tech`) | ✅ | |
| System (entity, migration, CLI create/list/show) | ✅ | |
| Immutable Release + DeployRelease engine | ✅ | DeploySource removed in v0.2 |
| OCI adapter (pull + digest) + `app deploy --image` | ✅ | |
| Manifest v2 with `[delivery]` + repository enforcement | ✅ | Replaced by schema v3 in v0.2 |

**Capabilities removed in v0.2:**
- ~~`app deploy-source`~~ (local build removed)
- ~~`deployment_deploy_source.rs`~~ (local-build engine removed)
- ~~`local_build`~~ (local-build module removed)
- ~~`[source]` and `[build]` in the manifest~~ (removed in schema v3)
- ~~Import by local path~~ (remote Git only in v0.2)

### Pending — 7 Deliveries

#### 1. System

- [x] `System` entity (id, name, description?, created_at)
- [x] `Application.system_id`
- [x] Migration `0005_systems`
- [x] CLI: `pneuma system create`, `system list`, `system show`

#### 2. Release + Deployment Engine Refactoring

Introduce Release, replace candidate/current/previous, and split
`deployment_deploy_internal.rs` (~1500 lines) into two paths with clear
responsibilities.

**Domain:**

- [x] `Release` entity (id, application_id, image_repository, image_digest, source_revision?, created_at)
- [x] Migration `0006_releases`
- [x] Migration `0007_deployment_release` (deployment references release, no longer revision)
- [x] Remove `RuntimeRole` (candidate/current/previous); RuntimeInstance gains its own states: `starting | running | stopped | failed | removed`
- [x] `Application.active_deployment_id` replaces roles; active deployment → active release → active runtime
- [x] Deployment states: `pending | starting | verifying | activating | succeeded | failed` (remove `preparing_source`, `building`, `switching_traffic`, `verifying_external`)
- [x] Rollback creates a new deployment (type=rollback) from the prior Release; it does not depend on a prior container existing

**Engine split:**

- [x] `DeployRelease` (`deployment_deploy_release.rs`): ensure image → create deployment → create runtime → start → verify → activate
- [x] Remove `reconcile_existing_runtime()` from deployment; same active Release → no-op, stopped app → `app start`, prior release → rollback
- [x] Simplify `DeploymentSpecification`: no containerfile/context; only application_id, image, container_port, health_path, expected_status, visibility

**New use_cases structure:**

```text
use_cases/
├── release_create.rs          ← creates Release (OCI)
├── deployment_deploy_oci.rs   ← DeployOci: pull/verify → Release → DeployRelease
├── deployment_deploy_branch.rs← DeployByBranch: branch → commit → image tag → DeployOci
├── deployment_deploy_release.rs ← DeployRelease: linear orchestrator
├── deployment_start_candidate.rs ← candidate runtime creation
├── deployment_activate_public.rs ← public activation (health + Caddy)
├── deployment_runtime_cleanup.rs ← cleanup of candidates and old runtimes
├── deployment_progress.rs        ← progress reporting
├── deployment_transition.rs   ← persisted state machine
├── deployment_rollback.rs     ← rollback as a new deployment
├── application_runtime.rs     ← start/stop/status lifecycle
├── exposure_change.rs         ← public ↔ internal without redeployment
└── ...
```

**Note:** `deployment_deploy_source.rs` was removed in v0.2 along with local build.

#### 3. OCI adapter

- [x] OCI adapter: `podman pull`, `podman image inspect`, validate that the digest matches the requested one
- [x] `DeployRelease` accepts a registry image (in addition to a local build image)
- [x] CLI: `pneuma app deploy <app> --image <repo@sha256:...>` as the official path
- [x] Reject mutable tags (require digest)

#### 4. deploy-source (CLI) — REMOVED IN v0.2

- [x] CLI: `pneuma app deploy-source <app> <repo> --revision <rev>` (alternative path)
- [x] Single engine: `DeploySource` was already created in delivery 2; expose it only in the CLI here

**Note:** This path was removed in v0.2. The only deployable artifact is now `image@digest` discovered by CI.

#### 5. Manifest with `[delivery]` — EVOLVED TO SCHEMA v3 IN v0.2

- [x] `[delivery]` section in the manifest: `type = "oci"`, `image = "ghcr.io/..."`
- [x] `[source]` and `[build]` become optional (only for deploy-source)
- [x] `schema_version = 2`
- [x] Persist `application_delivery_specs` during import
- [x] `app deploy --image` rejects a repository different from the permitted one; `deploy-source` requires `[source]`/`[build]`

**Note:** In v0.2, the schema evolved to v3, removing `[source]` and `[build]`. The repository comes from import and the branch comes from deployment.

#### 6. History + Visibility

- [x] History based on Release/digest (no longer commit_sha)
- [x] Output: `DEPLOYMENT | RELEASE | SOURCE | STATUS`
- [x] Rename CLI: `app expose` → `app visibility set <app> public|internal`
- [x] Output messages aligned with the term "visibility"

#### 7. Final Operability

- [x] Survives host reboot (Quadlet per deployment, enabled after promotion)
- [x] Extended doctor: working rootless mode, `caddy validate`, active OCI pull, and disk space
- [x] `pneuma database backup <path>`
- [x] `pneuma database restore <path>`
- [x] Updated docs (roadmap, architecture, scope, README) reflecting OCI-first
- [x] Final E2E: CI → GHCR → pull → deploy → health → active → rollback → reboot

**v0.1.0 completed on August 8, 2026** — all acceptance criteria were validated
on the production VPS (`srv655252`, Debian 13). v0.2 (Git-aware OCI Delivery)
was completed next; see the following section.

### Target Data Model (v0.1)

```text
System
  id, name, description?, created_at

Application
  id, system_id, name, desired_runtime_state, desired_visibility,
  active_deployment_id, runtime_config, health_config, created_at, updated_at

Release
  id, application_id, image_repository, image_digest,
  source_revision?, created_at

Deployment
  id, application_id, release_id, type, status,
  requested_by, started_at, finished_at, failure_reason

RuntimeInstance
  id, deployment_id, runtime_identifier,
  state (starting|running|stopped|failed|removed),
  host_address, host_port, created_at
```

---

## v0.2 — Git-aware OCI Delivery

**Status:** completed on August 10, 2026

Pneuma moves from "operates an OCI image it receives" to "finds the artifact for
a branch commit and deploys it." Pneuma no longer builds applications: **CI
produces artifacts, Pneuma discovers and operates artifacts.**

```text
Git branch → commit → OCI digest → Release → deployment
```

Principles and structural changes:

- **Remove local build:** `app deploy-source`, `deployment_deploy_source`,
  `local_build`, `[build]`, `application_build_specs`, and permanent build
  checkout. The only deployable artifact is `image@digest`.
- **Remote Git import:** `pneuma app import <git-url> [--manifest <path>]`
  supports a temporary checkout (clone → read `pneuma.toml` → persist → remove).
  `import` does not create a deployment; `active_deployment_id = null`, desired
  runtime = stopped.
- **Manifest schema v3:** no `[source]`/`[build]`. Repository comes from import,
  branch comes from deployment, and OCI/runtime/exposure come from the manifest.
  Convention: `deploy/<environment>/pneuma.toml` (dev/staging/production);
  environments are not yet domain entities.
- **Persistence:** architectural rule — use cases decide what must happen, and
  SQLite stores decide how to persist atomically. `SqliteApplicationStore`,
  `SqliteDeploymentStore`, `SqliteRuntimeStore`, `SqliteReleaseStore`
  (capability-oriented, not a repository per table). Simple reads (`app list`,
  `system list`, history) remain direct queries. Never open a transaction during
  Git/registry/Podman/Caddy (external I/O outside the transaction; persist in a
  short transaction at the end).
- **Deploy by branch:** `pneuma app deploy <app> --branch <branch>`
  (mutually exclusive with `--image`). New `DeployByBranch` use case
  (`deployment_deploy_branch.rs`): branch → `git ls-remote` → `CommitSha`
  (fixed for the deployment) → `image:<commit-sha>` convention → resolve tag →
  digest → `DeployOci`. If CI has not yet published the artifact →
  `ArtifactNotFound`, with no fallback to `:latest`/prior/local build.
- **Release correlates source and artifact:** `source_revision`, `image_repository`,
  `image_digest`, `image_reference`.
- **Implementation phases (all completed):**

  - A — simplify: remove `deploy-source`, `deployment_deploy_source`,
    `local_build`, `[build]`, `application_build_specs`, local source, and
    permanent checkout.
  - B — separate persistence: create the four SQLite stores and migrate
    create/transition/fail/promotion, runtime persistence, and release/rollback.
  - C — new schema: manifest v3, `deploy/<environment>/pneuma.toml`, new
    migrations (never alter historical ones).
  - D — remote Git import: `app import <git-url>`, `--manifest`, temporary
    checkout, persist `repository_url`/`manifest_path`, idempotence.
  - E — Git resolution: remote Git adapter, `resolve_branch()`, `CommitSha`,
    authentication/repository/branch errors.
  - F — OCI discovery: `image:<commit>` convention, resolve the commit tag →
    digest, never return a mutable tag to the engine.
  - G — deploy by branch: `DeployByBranch`, `--branch`, mutual exclusion with
    `--image`, persist `source_revision`.
  - H — real application: move website manifests, import staging, test
    `--branch staging`, automate staging in Actions, import production, test
    `--branch main`, and rollback.

**Definition of Done:** `pneuma app import <git-url> --manifest
deploy/staging/pneuma.toml` followed by `pneuma app deploy
vitoralmeida-tech-staging --branch staging` finds and deploys the correct
artifact — without manual cloning on the VPS, path import, local build, `podman
build` by Pneuma, manual digest discovery, or manual Caddy editing.

---

## v0.3 — Consolidation and Operational Hardening

**Status:** completed on August 13, 2026

v0.3 consolidates the domain and persistence model and hardens host operations
before reconciliation. It introduces a breaking import-contract change: `pneuma
app import` accepts remote Git URLs only; local paths are rejected, while
`file://` remains available for local test repositories.

- Deployment and RuntimeInstance are first-class domain types.
- Release represents only the immutable OCI artifact; source provenance belongs
  to Deployment.
- Application import, runtime lifecycle, and deployment creation persist through
  capability-oriented SQLite stores.
- Bootstrap reruns, rootless Podman account invariants, Caddy replacement, and
  CI deploy-key provisioning are reproducible and validated.
- Disposable Debian 13 bootstrap and E2E regressions prove candidate failure
  preservation, rollback, reboot recovery, local HTTPS, CI SSH boundaries, and
  semantic SQLite restore.
- CI runs pinned ShellCheck and shfmt for tracked shell scripts.

---

## v0.4 — Reconciliation & Deployment Reliability

With Git/source/artifact well defined, Pneuma evolves from command-driven to
declarative (desired vs. observed state). `pneuma reconcile` observes the
materialized state (Podman/systemd, Caddy) and converges it toward the desired
state persisted in SQLite, without changing intent or creating a Release/Deployment.

- [ ] Desired vs observed state
- [ ] Drift detection and automatic recovery
- [ ] Deployment recovery
- [ ] Better restart/reboot convergence
- [ ] Candidate/activation improvements
- [ ] Deployment mutual exclusion (one per Application at a time)
- [ ] Non-interactive CLI (`--non-interactive`, structured output, exit codes)

Already delivered outside v0.4 (not future work):

- GitHub Actions validation (format, lint, test, build) and GHCR build/push
  published as `image:<commit-sha>` — active CI pipeline.
- SSH deployment through a restricted dispatcher: GitHub Actions → dedicated key
  → `pneuma` user (no password, no sudo), `authorized_keys` with a forced command
  (`pneuma ci dispatch`) limited to `deploy <app> <branch>` and `version`.

Out of scope until a demonstrated need: registry watcher (deploy when the branch
artifact becomes available) and automatic deployment policies. No complete audit,
generic `--idempotency-key`, image retention, or automatic rollback after
promotion at this stage — candidate failure before promotion already preserves
the active version; a decision to automatically revert an already promoted
version is deferred to an explicit future policy.

---

## v0.5 — Application Topology & Internal Networking

Add relationships between Applications: Pneuma understands how applications
relate to each other, not just how each runs in isolation.

- [ ] Service relationships (`Application A depends on Application B`)
- [ ] Internal services
- [ ] Application dependencies
- [ ] Network/service addressing
- [ ] System as a real grouping mechanism
- [ ] Basic service discovery

---

## v0.6 — Network Policy Enforcement

Relationships declared in v0.5 feed connectivity policies applied on the host.

### Network enforcement

- [ ] `pneuma-netd` (nftables, default deny, explicit connectivity)

---

## v0.7 — Workload Identity & Secure S2S

Identity per workload; communication between applications becomes authenticated
and authorized.

- [ ] SPIFFE + SPIRE (each `RuntimeInstance` receives its own identity)
- [ ] `pneuma-proxy` per `RuntimeInstance` (mTLS, authn, authz, telemetry)

---

## v0.8 — Artifact Security & Secrets

Artifact lifecycle security and application secrets.

### Artifact security

- [ ] SBOM generation and enforcement
- [ ] Image signature verification (cosign/Notation)
- [ ] Admission policies (reject unsigned images)
- [ ] Secret management (injection, rotation)
- [ ] Implemented threat model

---

## Out of Scope (frozen beyond v0.8)

TUI, HTTP API, webhooks, centralized observability, multiple hosts, scheduler,
remote agents, distributed reconciliation, declarative communication between
apps, dependencies, service discovery beyond the v0.5 basics, managed builds as
an official feature, canary, gradual rollout, autoscaling, Kubernetes, RBAC,
multi-user support.

Network, identity, S2S, and artifact-security items leave the freeze in the
versions that introduce them (v0.6 through v0.8); everything else is explicitly
unfrozen in a future version.
