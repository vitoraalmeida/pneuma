# Changelog

## v0.4.2 — Domain Type Closure (2026-08-21)

### Changed

- Runtime lifecycle use cases (`status`, `start`, `stop`, candidate cleanup) now
  accept `ApplicationName` instead of raw strings.
- `OciArtifact` is parsed once at the CLI edge for `--image` deploys and once at
  the branch-resolution boundary; `deploy_oci*`, `pull_image`, and rollback
  consume the typed artifact without re-parsing.
- Container observation (`observe_container`), container-id resolution
  (`resolve_container_id`), named-container observation, external health checks,
  and Quadlet unit rendering accept typed domain identities (`ContainerId`,
  `ContainerPort`, `DomainName`, `HealthCheckPath`, `HealthCheckStatus`,
  `ApplicationName`, `DeploymentId`, `OciArtifact`). Duplicated validation was
  removed from those adapters.
- Introduced `HostPort`, a newtype for the reserved loopback host port, carried
  by `StartedCandidate`, `ExpectedRuntimeEndpoint`, and Quadlet unit creation.
  Persisted representation remains `u16`.

## v0.4.0 — Reconciliation and Domain Boundaries (2026-08-21)

### Added

- `pneuma reconcile <application>`: read-only observation of Podman, Quadlet,
  and Caddy state against persisted intent, with `no-op` and `deferred` results.
- Runtime drift recovery: recreates missing Quadlet containers and rematerializes
  absent units only when persisted identity, digest, labels, and loopback
  endpoint are unambiguous; divergent identity requires manual intervention.
- Exposure drift recovery: repairs confirmed public fragments through validated
  Caddy reload and external health, removes internal routes, and records
  `failed`, `diverged`, or manual outcomes when convergence cannot be confirmed.
- Interrupted deployment recovery per stage, cleaning only resources with proven
  candidate identity while preserving the active healthy runtime and route.
- Per-Application operation locking serializing reconcile and deployment effects.
- Step-by-step stderr progress for `pneuma app deploy --verbose`.

### Changed

- Domain ownership realignment without SQLite or CLI behavior changes: Git
  commit identity, repository classification, and source locations live in the
  Git domain; container identity lives in the Runtime domain; System naming,
  Application runtime intent, and runtime/health specifications live in their
  owning modules.
- Manifest validation produces a typed import projection consumed directly by
  the import transaction instead of revalidated raw strings.
- Store APIs retain typed logical identities (`ApplicationId`, `SystemId`,
  `ReleaseId`, `DeploymentId`, `RuntimeInstanceId`) and names until the SQLite
  parameter boundary; compare-and-set writes report explicit stale outcomes.

## v0.3.1 — Caddy Routing Fix (2026-08-13)

### Fixed

- Unmatched HTTP hosts return a generic `404 Not Found`, including after a
  public Application becomes internal.
- Bootstrap reruns correctly identify Caddy as the owner of ports 80/443.

### Changed

- Internalized Applications remove their public Caddy route while their
  loopback runtime remains running.

## v0.3.0 — Consolidation and Operational Hardening (2026-08-13)

### Added

- Reproducible bootstrap and full E2E regression on disposable Debian 13 VMs.
- Pinned ShellCheck and shfmt checks for every tracked shell script in CI.
- First-class `Deployment` and `RuntimeInstance` domain types.
- Capability-oriented persistence for application import, runtime lifecycle, and
  deployment creation.

### Breaking Changes

- `pneuma app import` accepts remote Git URLs only. Local paths are rejected;
  `file://` remains available for local test repositories.

### Fixed

- Bootstrap reruns, Caddy configuration replacement, rootless Podman account
  invariants, and CI deploy-key provisioning.
- Candidate failure preservation, rollback, reboot recovery, local HTTPS,
  restricted CI SSH boundaries, and SQLite restore coverage in E2E.
- Re-import reports the persisted deployment state; rollback preserves source
  provenance; visibility and runtime state remain correct after failed effects.

### Changed

- Documentation now distinguishes production smoke tests from disposable VM
  bootstrap and E2E regression.
- Deployment source provenance is stored on `Deployment`, while `Release`
  represents only the immutable OCI artifact.

## v0.2.0 — Git-aware OCI Delivery (2026-08-11)

Pneuma moves from "operates an OCI image I provide" to "finds the artifact for
a branch commit and deploys it". Local builds were removed: **CI produces
artifacts; Pneuma discovers and operates artifacts.**

### Added

- `pneuma app import <git-url> --manifest <path>`: imports applications from
  remote Git repositories, with a temporary checkout and persistence of
  `repository_url`/`manifest_path`.
- `pneuma app deploy <app> --branch <branch>`: resolves the branch commit via
  `git ls-remote`, discovers the OCI image using the `image:<commit>` convention,
  and deploys the immutable artifact (mutually exclusive with `--image`).
- `pneuma ci` (restricted SSH dispatcher): accepts only `deploy <app> <branch>`
  and `version` through `SSH_ORIGINAL_COMMAND`, with injection-resistant validation.
- Manifest schema v3 without `[source]`/`[build]`; convention
  `deploy/<environment>/pneuma.toml`.
- Capability-oriented SQLite stores (`SqliteApplicationStore`,
  `SqliteDeploymentStore`, `SqliteRuntimeStore`, `SqliteReleaseStore`).
- Migration 0013 for `application_sources` v3.
- VPS bootstrap with pre-flight checks (Debian 13, internet/DNS, disk, memory,
  CPU, conflicting services, ports 80/443) and final-state validation.
- Restricted CI deploy-key provisioning during bootstrap
  (`--ci-public-key`), with forced command `pneuma ci dispatch`.
- Development VM E2E battery (`scripts/dev-vm/e2e.sh`) and VPS bootstrap test
  (`scripts/test-bootstrap-vps.sh`).
- Development VM scripts and tutorial (`scripts/dev-vm/`,
  `docs/operations/dev-vm-tutorial.md`).

### Changed

- Removed local builds: `app deploy-source`, `local_build`, `[build]`, and
  consumption of a permanent checkout.
- Migrated the CLI to `clap` derive.
- Decoupled the Pneuma environment from the login shell (`/etc/pneuma/environment`).
- Refactored deployment: extracted candidate startup, public activation, runtime
  cleanup, and progress reporting; explicitly modeled candidate resource lifecycle.
- Derived `XDG_RUNTIME_DIR`/`DBUS_SESSION_BUS_ADDRESS` from the effective uid.

### Fixed

- `app status`/`stop`/`start` failed after container removal.
- `visibility internal` failed with `NULL` in the domain.
- Cleaned up systemd units after candidate failures.
- Multiple bootstrap fixes (directory ownership, doctor cwd, Podman warnings,
  operation ordering).

## v0.1.0 — OCI Foundation (2026-08-08)

OCI-first deployments: immutable image pulls, rootless Quadlet runtime, health
checks, Caddy exposure, rollback, and VPS operations.
