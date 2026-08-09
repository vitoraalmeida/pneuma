# Pneuma

[![CI](https://github.com/vitoraalmeida/pneuma/actions/workflows/ci.yml/badge.svg)](https://github.com/vitoraalmeida/pneuma/actions/workflows/ci.yml)

Single-host deployment CLI for containerized applications.

## Overview

Pneuma imports containerized app repositories (declared by a `pneuma.toml` manifest), deploys immutable OCI releases with rootless Podman on loopback, health-checks them, and exposes public apps through Caddy. State lives in SQLite. Successful deployments are supervised by rootless Podman Quadlet units and survive a host reboot.

Designed for personal sites and small projects that need production-grade deployment without the complexity of Kubernetes or multi-host orchestration.

## How it works

```
pneuma.toml manifest
    ↓
CI builds a container image and pushes it to a registry
    ↓
pneuma app import <repository-path>
    ↓
pneuma app deploy <app> --image ghcr.io/owner/app@sha256:...
    ↓
Pull image → Container create → Health check → Promote release
    ↓
Caddy reverse proxy (if public)
```

## Features

- **OCI-first**: deploy digest-pinned releases declared by `[delivery]`; `deploy-source` remains available for local builds
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
- **Git** for `deploy-source` builds and imports

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
bash scripts/bootstrap-vps.sh <pneuma-source-url> [application-repository-url]
```

See [`docs/operations/vps-bootstrap.md`](docs/operations/vps-bootstrap.md) for the full Debian 13 guide, and `scripts/bootstrap-vps.sh` for prerequisites.

For a step-by-step walkthrough from a fresh VPS to a deployed site, see [`docs/usage-guide.md`](docs/usage-guide.md).

## Quick start

1. **Write a manifest** in your application repository:

```toml
# pneuma.toml
schema_version = 2

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

2. **Import the application**:

```bash
pneuma app import /path/to/my-app
```

3. **Deploy an immutable image**:

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
| `pneuma app import <repository-path>` | Import an application from a local repository |
| `pneuma app list` | List all registered applications |
| `pneuma app deploy <app> --image <repository@sha256:...>` | Deploy an immutable OCI release |
| `pneuma app deploy-source <app> <repository-path> --revision <revision>` | Build and deploy a local source revision |
| `pneuma app visibility set <app> <public\|internal>` | Set desired public visibility |
| `pneuma app status <app>` | Show desired and observed runtime state |
| `pneuma app start <app>` | Start a stopped application |
| `pneuma app stop <app>` | Stop a running application |
| `pneuma app deployments <app>` | List deployment history |
| `pneuma deployment rollback <app>` | Roll back to the previous release |
| `pneuma database backup <path>` | Create a consistent SQLite backup |
| `pneuma database restore <path>` | Validate and restore a SQLite backup |
| `pneuma version` | Print version |
| `pneuma doctor` | Verify host prerequisites |

Add `--verbose` before the command to see step-by-step progress.

## Manifest

The `pneuma.toml` manifest declares application configuration:

```toml
schema_version = 2

[application]
name = "personal-site"

[delivery]
type = "oci"
image = "ghcr.io/user/personal-site"

[runtime]
container_port = 8080
healthcheck_path = "/healthz"
expected_status = 200

[exposure]
default_visibility = "public"
domain = "example.com"
```

**Fields:**

- `schema_version`: manifest schema version (currently `2`)
- `name`: application identifier (used in all commands)
- `delivery.type`: delivery model (`oci`); the image is produced by CI
- `delivery.image`: OCI repository that CI pushes immutable images to
- `containerfile`: path to Containerfile relative to context (only for `deploy-source`)
- `context`: build context directory (only for `deploy-source`)
- `container_port`: port exposed by the container
- `healthcheck_path`: HTTP path for health checks
- `expected_status`: expected HTTP status code (typically 200)
- `default_visibility`: `internal` or `public`
- `domain`: required for public apps, ignored for internal

The `[source]` and `[build]` sections are only needed for `deploy-source` local builds; they must be provided together.

## Configuration

All runtime paths come from environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `PNEUMA_DATABASE_PATH` | `/var/lib/pneuma/database/pneuma.sqlite3` | SQLite database location |
| `PNEUMA_WORKSPACE_PATH` | `/var/lib/pneuma/checkouts` | Git checkout directory |
| `PNEUMA_CADDY_MANAGED_PATH` | `/etc/caddy/applications` | Caddy fragment directory |
| `PNEUMA_CADDYFILE_PATH` | `/etc/caddy/Caddyfile` | Main Caddyfile location |

## Project structure

```
pneuma/
├── src/
│   ├── main.rs                      # CLI entry point
│   ├── lib.rs                       # Module declarations
│   ├── domain/                      # Pure domain types
│   │   ├── application.rs           # Application model
│   │   ├── manifest.rs              # Manifest parsing
│   │   ├── release.rs               # Release model
│   │   └── system.rs                # System model
│   ├── use_cases/                   # Business logic
│   │   ├── application_import.rs    # Application import
│   │   ├── application_list.rs      # Application list
│   │   ├── application_runtime.rs   # Lifecycle management
│   │   ├── deployment_create.rs     # Deployment creation
│   │   ├── deployment_deploy_oci.rs # OCI image deployment entry point
│   │   ├── deployment_deploy_release.rs # Deployment orchestrator
│   │   ├── deployment_deploy_source.rs  # Local source builds
│   │   ├── deployment_transition.rs # State machine
│   │   └── ...                      # Other use cases
│   └── adapters/                    # External integrations
│       ├── git_source.rs            # Git adapter
│       ├── local_build.rs           # Podman build
│       ├── local_runtime.rs         # Container lifecycle
│       ├── oci_image.rs             # OCI image pull
│       ├── caddy_exposure.rs        # Caddy integration
│       ├── health_check.rs          # Internal health checks
│       ├── external_health.rs       # External health checks
│       ├── systemd_quadlet.rs       # Quadlet unit management
│       ├── port_allocator.rs        # Runtime port allocation
│       └── database.rs              # SQLite and migrations
├── migrations/                      # Versioned SQL migrations
├── scripts/                         # Operational scripts
│   └── bootstrap-vps.sh             # VPS setup script
├── tests/                           # Integration tests
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
- **v0.2** (planned): automatic deployments triggered by CI
- **v0.3** (planned): GitHub Actions integration via SSH

See `docs/roadmap.md` for the full product vision.

## License

Copyright (C) 2026 Vitor Almeida

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
