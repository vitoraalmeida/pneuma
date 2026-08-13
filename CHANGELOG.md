# Changelog

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
- E2E test battery (`scripts/test-battery.sh`) and VPS bootstrap test
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
