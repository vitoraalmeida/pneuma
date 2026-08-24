# Pneuma

[![CI](https://github.com/vitoraalmeida/pneuma/actions/workflows/ci.yml/badge.svg)](https://github.com/vitoraalmeida/pneuma/actions/workflows/ci.yml)

Pneuma is a Single-host deployment Tool (song) for containerized applications.

## Why Pneuma?

Running a container is easy. Safely replacing it, knowing exactly which artifact
is active, validating it before traffic switches, preserving deployment history,
surviving reboots, rolling back, and reproducibly provisioning the host is the
harder problem Pneuma addresses.

Pneuma deliberately solves that problem for a small set of applications on one
Linux host. It is not a Kubernetes replacement or a multi-host orchestrator.

## What It Does

- Imports application configuration from a Git repository and `pneuma.toml`.
- Resolves a branch or tag to a full commit SHA, then deploys the resulting
  digest-pinned OCI artifact.
- Creates rootless, loopback-bound Podman runtimes through Quadlet and systemd.
- Health-checks candidates before promotion and preserves the active version when
  a replacement fails.
- Materializes public routes through Caddy while keeping runtime and visibility
  independent.
- Stores durable logical intent and Deployment history in SQLite.

## How It Works

```text
application repository -> CI -> OCI registry
        |                         |
        | pneuma.toml              | repository@sha256:...
        v                         v
    Pneuma CLI -> SQLite -> Quadlet/systemd -> rootless Podman -> application
                         -> Caddy -> Internet
```

Pneuma is invoked to make decisions and apply controlled effects, then exits.
systemd owns the promoted runtime afterward.

## Principles

- Applications survive Pneuma.
- Releases are immutable OCI digests; Deployments are activation attempts.
- Healthy active versions survive failed candidates.
- Desired intent and observed external state have different authorities.
- Public exposure is separate from runtime lifecycle.
- Containers run rootlessly and CI receives a narrow SSH deployment interface.
- Host setup and regression testing are reproducible.

## Quick Start

Build and install Pneuma on a Debian 13 host with rootless Podman, Caddy, and
Git available:

```bash
git clone https://github.com/vitoraalmeida/pneuma.git
cd pneuma
cargo build --release
sudo install -m 0755 target/release/pneuma /usr/local/bin/
```

Import an application repository, then deploy the artifact for a branch:

```bash
pneuma app import https://github.com/user/my-app --manifest deploy/staging/pneuma.toml
pneuma app deploy my-app --branch staging
pneuma app status my-app
```

For production setup, including bootstrap, CI keys, Caddy, and GitHub Actions,
follow [`docs/getting-started.md`](docs/getting-started.md).

## Documentation

| Need | Read |
|---|---|
| Understand the problem and scope | [`docs/architecture/system-context.md`](docs/architecture/system-context.md) |
| Understand implemented behavior | [`docs/architecture/architecture.md`](docs/architecture/architecture.md) |
| Understand persistence | [`docs/architecture/data-model.md`](docs/architecture/data-model.md) |
| Understand trust boundaries | [`docs/architecture/security-model.md`](docs/architecture/security-model.md) |
| Understand architecture threats | [`docs/architecture/threat-model.md`](docs/architecture/threat-model.md) |
| Understand architectural rationale | [`docs/decisions/`](docs/decisions/) |
| Set up and operate a host | [`docs/getting-started.md`](docs/getting-started.md) |
| Validate on a disposable VM | [`docs/operations/dev-vm-tutorial.md`](docs/operations/dev-vm-tutorial.md) |
| See active work and future plans | [`docs/iterations/current-iteration.md`](docs/iterations/current-iteration.md), [`docs/roadmap.md`](docs/roadmap.md) |
| Navigate all documentation | [`docs/README.md`](docs/README.md) |

## Status

v0.4.2 is the latest release. The next stage, v0.5 host observation
(observed state), is planned but not started. See
[`docs/roadmap.md`](docs/roadmap.md) for direction and
[`CHANGELOG.md`](CHANGELOG.md) for released changes.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --workspace --release
```

Follow [`docs/rust-guidelines.md`](docs/rust-guidelines.md) for code conventions.

## License

Copyright (C) 2026 Vitor Almeida

Pneuma is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.

Pneuma is distributed without any warranty; without even the implied warranty of
merchantability or fitness for a particular purpose. See the GNU GPL for details.
