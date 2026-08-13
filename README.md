# Pneuma

[![CI](https://github.com/vitoraalmeida/pneuma/actions/workflows/ci.yml/badge.svg)](https://github.com/vitoraalmeida/pneuma/actions/workflows/ci.yml)

Single-host deployment CLI for containerized applications.

## Overview

Pneuma imports containerized app repositories (declared by a `pneuma.toml` manifest), deploys immutable OCI releases with rootless Podman on loopback, health-checks them, and exposes public apps through Caddy. State lives in SQLite. Successful deployments are supervised by rootless Podman Quadlet units and survive a host reboot.

Designed for personal sites and small projects that need production-grade deployment without the complexity of Kubernetes or multi-host orchestration.

## How it works

```
pneuma.toml manifest (in Git repository)
    ↓
CI builds a container image and pushes it to a registry (tagged with commit SHA)
    ↓
pneuma app import <git-url> --manifest deploy/staging/pneuma.toml
    ↓
pneuma app deploy <app> --branch <branch>
    ↓
Resolve branch → commit SHA → discover image tag → pull image
    ↓
Container create → Health check → Promote release
    ↓
Caddy reverse proxy (if public)
```

## Features

- **OCI-first**: deploy digest-pinned releases declared by `[delivery]`
- **Rootless containers**: runs on Podman without root privileges
- **Health checks**: internal (loopback) and external (public endpoint) verification
- **Atomic deployments**: candidate containers are validated before promotion; failed deployments preserve the previous version
- **Caddy integration**: automatic reverse proxy configuration for public apps
- **Lifecycle management**: start, stop, and status commands with idempotent operations
- **Deployment history**: track all deployment attempts with release, status, and timestamps
- **SQLite persistence**: all state in a single database file with versioned migrations and backup/restore commands

## Requirements

- **Rust** 1.85 or later (for building from source)
- **Podman** with rootless mode configured
- **Caddy** for public app exposure
- **Git** for application imports

## Installation

### Build from source

```bash
git clone https://github.com/vitoraalmeida/pneuma.git
cd pneuma
cargo build --release
sudo install -m 0755 target/release/pneuma /usr/local/bin/
```

### VPS bootstrap

For a complete VPS setup (Podman, Caddy, user creation, directories), use the bootstrap script:

```bash
bash scripts/bootstrap-vps.sh <pneuma-source-url> [--ci-public-key <path>]
```

The script verifies prerequisites (Debian 13, internet/DNS, disk space, memory,
free ports 80/443) before touching the system, provisions the `pneuma` user
(no sudo), rootless Podman, Caddy and the compiled binary, and runs
`pneuma doctor` at the end. Pass `--ci-public-key` with the public key of a CI
deploy key to install the restricted SSH dispatcher.

See `scripts/bootstrap-vps.sh` for prerequisites and usage instructions, and follow
[`docs/getting-started.md`](docs/getting-started.md) for the complete setup:
generating the CI deploy key, running the bootstrap, importing and deploying an
application, and configuring the GitHub Actions workflow.

For disposable bootstrap and full E2E regression procedures, including local
TLS, restricted CI SSH, reboot, and semantic database restore, see
[`docs/operations/dev-vm-tutorial.md`](docs/operations/dev-vm-tutorial.md) and
[`docs/operations/backup-and-restore.md`](docs/operations/backup-and-restore.md).

## Quick start

1. **Write a manifest** in your application repository at `deploy/<environment>/pneuma.toml`:

```toml
# deploy/staging/pneuma.toml
schema_version = 3

[system]
name = "my-system"

[application]
name = "my-app"

[delivery]
type = "oci"
image = "ghcr.io/user/my-app"

[runtime]
container_port = 8080
healthcheck_path = "/healthz"
expected_status = 200

[exposure]
default_visibility = "public"
domain = "my-app.example.com"
```

2. **Import the application** from a Git repository (`app import` accepts Git
   URLs only; local paths are rejected and `file://` is reserved for local test
   repositories). Pneuma clones the repository temporarily to read the manifest,
   then removes the checkout:

```bash
pneuma app import https://github.com/user/my-app --manifest deploy/staging/pneuma.toml
```

3. **Deploy by branch** (Pneuma discovers the artifact from the commit):

```bash
pneuma app deploy my-app --branch staging
```

Or **deploy by specific image digest** (manual discovery):

```bash
pneuma app deploy my-app --image ghcr.io/user/my-app@sha256:<digest>
```

4. **Check status**:

```bash
pneuma app status my-app
```

## Commands

| Command | Description |
|---------|-------------|
| `pneuma system create <name>` | Create a system to group applications |
| `pneuma system list` | List all systems |
| `pneuma system show <name>` | Show a system and its applications |
| `pneuma app import <git-url> [--manifest <path>]` | Import an application from a Git repository |
| `pneuma app list` | List all registered applications |
| `pneuma app deploy <app> --branch <branch>` | Deploy the artifact from a specific branch |
| `pneuma app deploy <app> --image <repository@sha256:...>` | Deploy a specific OCI image by digest |
| `pneuma app visibility set <app> <public\|internal>` | Set desired public visibility |
| `pneuma app status <app>` | Show desired and observed runtime state |
| `pneuma app start <app>` | Start a stopped application |
| `pneuma app stop <app>` | Stop a running application |
| `pneuma app deployments <app>` | List deployment history |
| `pneuma deployment rollback <app>` | Roll back to the previous release |
| `pneuma database backup <path>` | Create a consistent SQLite backup |
| `pneuma database restore <path>` | Validate and restore a SQLite backup |
| `pneuma ci dispatch` | CI dispatcher via SSH forced command (internal) |
| `pneuma version` | Print version |
| `pneuma doctor` | Verify host prerequisites |

Add `--verbose` before the command to see step-by-step progress.

## Manifest

The `pneuma.toml` manifest declares application configuration. Convention: place manifests at `deploy/<environment>/pneuma.toml` (e.g., `deploy/staging/pneuma.toml`, `deploy/production/pneuma.toml`):

```toml
# deploy/staging/pneuma.toml
schema_version = 3

[system]
name = "personal-website"

[application]
name = "vitoralmeida-tech-staging"

[delivery]
type = "oci"
image = "ghcr.io/user/personal-site"

[runtime]
container_port = 8080
healthcheck_path = "/healthz"
expected_status = 200

[exposure]
default_visibility = "public"
domain = "staging.example.com"
```

**Fields:**

- `schema_version`: manifest schema version (currently `3`)
- `system.name`: system identifier to group applications
- `application.name`: application identifier (used in all commands)
- `delivery.type`: delivery model (`oci`); the image is produced by CI
- `delivery.image`: OCI repository that CI pushes immutable images to
- `container_port`: port exposed by the container
- `healthcheck_path`: HTTP path for health checks
- `expected_status`: expected HTTP status code (typically 200)
- `default_visibility`: `internal` or `public`
- `domain`: required for public apps, ignored for internal

The repository URL comes from the `pneuma app import` command, not from the manifest; the branch comes from the `pneuma app deploy --branch` command. `app import` clones the repository only temporarily (read manifest, persist, remove) and rejects local paths; `file://` URLs are accepted for local test repositories.

## Configuration

All runtime paths come from environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `PNEUMA_DATABASE_PATH` | `/var/lib/pneuma/database/pneuma.sqlite3` | SQLite database location |
| `PNEUMA_WORKSPACE_PATH` | `/var/lib/pneuma/checkouts` | Git checkout directory |
| `PNEUMA_CADDY_MANAGED_PATH` | `/etc/caddy/applications` | Caddy fragment directory |
| `PNEUMA_CADDYFILE_PATH` | `/etc/caddy/Caddyfile` | Main Caddyfile location |
| `PNEUMA_RUNTIME_PORT_RANGE` | `30000-39999` | Host loopback port range for runtimes |
| `PNEUMA_QUADLET_DIR` | `$HOME/.config/containers/systemd` | Quadlet unit directory |

## Project structure

```
pneuma/
├── src/
│   ├── main.rs                      # CLI entry point (clap derive)
│   ├── lib.rs                       # Module declarations
│   ├── domain/                      # Pure domain types
│   │   ├── application.rs           # Application model
│   │   ├── manifest.rs              # Manifest parsing
│   │   ├── release.rs               # Release model
│   │   └── system.rs                # System model
│   ├── use_cases/                   # Business logic
│   │   ├── application_import.rs    # Application import (Git remote)
│   │   ├── application_list.rs      # Application list
│   │   ├── application_runtime.rs   # Lifecycle management
│   │   ├── ci_dispatch.rs           # SSH restricted dispatcher
│   │   ├── deployment_create.rs     # Deployment creation
│   │   ├── deployment_deploy_branch.rs # Deploy by branch (Git-aware)
│   │   ├── deployment_deploy_oci.rs # OCI image deployment entry point
│   │   ├── deployment_deploy_release.rs # Deployment orchestrator
│   │   ├── deployment_list.rs       # Deployment history
│   │   ├── deployment_transition.rs # State machine
│   │   ├── release_create.rs        # Release creation
│   │   └── ...                      # Other use cases
│   └── adapters/                    # External integrations
│       ├── git_source.rs            # Git adapter (remote resolution)
│       ├── local_runtime.rs         # Container lifecycle
│       ├── oci_image.rs             # OCI image pull + digest discovery
│       ├── caddy_exposure.rs        # Caddy integration
│       ├── health_check_internal.rs # Internal health checks
│       ├── health_check_external.rs # External health checks
│       ├── systemd_quadlet.rs       # Quadlet unit management
│       ├── port_allocator.rs        # Runtime port allocation
│       ├── stores/                  # SQLite capability stores
│       └── database.rs              # SQLite and migrations
├── migrations/                      # Versioned SQL migrations
├── scripts/                         # Operational scripts
│   ├── bootstrap-vps.sh             # VPS setup script
│   ├── test-bootstrap-vps.sh        # VPS bootstrap test
│   ├── verify-vps.sh                # VPS post-setup verification
│   └── dev-vm/                      # Development VM scripts and fixtures
├── tests/                           # Integration tests
├── CHANGELOG.md                     # Version history
└── docs/                            # Architecture and guidelines
```

## Development

Run all checks before committing:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

See `docs/rust-guidelines.md` for code conventions and `AGENTS.md` for contribution guidelines.

## Roadmap

- **v0.1** (released): OCI-first deployments — immutable image pulls, rootless Quadlet runtime, health checks, Caddy exposure, rollback, and VPS operations
- **v0.2** (released): Git-aware OCI delivery — deploy by branch, automatic artifact discovery from CI, manifest schema v3, SQLite stores for persistence
- **v0.3** (planned): reconciliation and deployment reliability — desired vs observed state, drift detection and recovery, deployment recovery, non-interactive CLI

See `docs/roadmap.md` for the full product vision.

## License

Copyright (C) 2026 Vitor Almeida

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
