# Pneuma

[![CI](https://github.com/vitoraalmeida/pneuma/actions/workflows/ci.yml/badge.svg)](https://github.com/vitoraalmeida/pneuma/actions/workflows/ci.yml)

Single-host deployment CLI for containerized applications.

## Overview

Pneuma imports containerized app repositories (declared by a `pneuma.toml` manifest), builds and runs them with rootless Podman on loopback, health-checks them, and exposes public apps through Caddy. State lives in SQLite.

Designed for personal sites and small projects that need production-grade deployment without the complexity of Kubernetes or multi-host orchestration.

## How it works

```
pneuma.toml manifest
    ↓
pneuma app import <repository>
    ↓
pneuma app deploy <app> <repository> --revision <commit>
    ↓
Git checkout → Podman build → Container create → Health check
    ↓
Promote to current runtime
    ↓
Caddy reverse proxy (if public)
```

## Features

- **Manifest-driven**: declare application name, source, build, runtime, and exposure in `pneuma.toml`
- **Rootless containers**: runs on Podman without root privileges
- **Health checks**: internal (loopback) and external (public endpoint) verification
- **Atomic deployments**: candidate containers are validated before promotion; failed deployments preserve the previous version
- **Caddy integration**: automatic reverse proxy configuration for public apps
- **Lifecycle management**: start, stop, and status commands with idempotent operations
- **Deployment history**: track all deployment attempts with commit, status, and timestamps
- **SQLite persistence**: all state in a single database file with versioned migrations

## Requirements

- **Rust** 1.85 or later (for building from source)
- **Podman** with rootless mode configured
- **Caddy** for public app exposure
- **Git** for source repository operations

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

See `scripts/bootstrap-vps.sh` for prerequisites and details.

## Quick start

1. **Write a manifest** in your application repository:

```toml
# pneuma.toml
schema_version = 1

[application]
name = "my-app"

[source]
repository = "https://github.com/user/my-app"
branch = "main"

[build]
containerfile = "Containerfile"
context = "."

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

3. **Deploy a revision**:

```bash
pneuma app deploy my-app /path/to/my-app --revision abc1234
```

4. **Check status**:

```bash
pneuma app status my-app
```

## Commands

| Command | Description |
|---------|-------------|
| `pneuma app import <repository-path>` | Import an application from a local repository |
| `pneuma app list` | List all registered applications |
| `pneuma app deploy <app> <repository> --revision <rev>` | Deploy a specific revision |
| `pneuma app status <app>` | Show desired and observed runtime state |
| `pneuma app start <app>` | Start a stopped application |
| `pneuma app stop <app>` | Stop a running application |
| `pneuma app deployments <app>` | List deployment history |

Add `--verbose` before the command to see step-by-step progress.

## Manifest

The `pneuma.toml` manifest declares application configuration:

```toml
schema_version = 1

[application]
name = "personal-site"

[source]
repository = "https://github.com/user/personal-site"
branch = "main"

[build]
containerfile = "Containerfile"
context = "."

[runtime]
container_port = 8080
healthcheck_path = "/healthz"
expected_status = 200

[exposure]
default_visibility = "public"
domain = "example.com"
```

**Fields:**

- `name`: application identifier (used in all commands)
- `repository`: Git repository URL
- `branch`: default branch for reference
- `containerfile`: path to Containerfile relative to context
- `context`: build context directory
- `container_port`: port exposed by the container
- `healthcheck_path`: HTTP path for health checks
- `expected_status`: expected HTTP status code (typically 200)
- `default_visibility`: `internal` or `public`
- `domain`: required for public apps, ignored for internal

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
│   │   └── manifest.rs              # Manifest parsing
│   ├── use_cases/                   # Business logic
│   │   ├── deploy_internal_revision.rs  # Deployment orchestrator
│   │   ├── import_application.rs    # Application import
│   │   ├── application_runtime.rs   # Lifecycle management
│   │   ├── create_deployment.rs     # Deployment creation
│   │   ├── transition_deployment.rs # State machine
│   │   └── ...                      # Other use cases
│   └── adapters/                    # External integrations
│       ├── git_source.rs            # Git adapter
│       ├── local_build.rs           # Podman build
│       ├── local_runtime.rs         # Container lifecycle
│       ├── caddy_exposure.rs        # Caddy integration
│       ├── health_check.rs          # Internal health checks
│       ├── external_health.rs       # External health checks
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

- **v0.1** (current): Import and deploy from source with health checks and Caddy exposure
- **v0.2**: Build images in CI, deploy by OCI digest
- **v0.3**: Automatic deployment triggered by GitHub Actions via SSH

See `docs/roadmap.md` for the full product vision.

## License

Copyright (C) 2026 Vitor Almeida

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
