# Pneuma Documentation

Index of documents and their status. Rule: a **living** document describes the
system or current work and must be updated in the same change that alters it;
a **record** describes completed work and does not change.

## Living Documents

| Document | Contents |
|---|---|
| [`rust-guidelines.md`](rust-guidelines.md) | Mandatory Rust code conventions |
| [`getting-started.md`](getting-started.md) | Complete Debian 13 VPS setup: CI keys, bootstrap, import, deployment, and GitHub Actions |
| [`operations/dev-vm-tutorial.md`](operations/dev-vm-tutorial.md) | Development VM tutorial (Debian 13): creation, provisioning, deployment, and E2E |
| [`architecture/architecture.md`](architecture/architecture.md) | Implemented architecture: structure, Quadlet runtime, exposure, persistence, and state machine |
| [`architecture/data-model.md`](architecture/data-model.md) | Implemented SQLite data model, entity relationships, state, and persistence invariants |
| [`design/reconciliation.md`](design/reconciliation.md) | Approved reconciliation semantics for v0.4 |
| [`design/reconciliation-e2e.md`](design/reconciliation-e2e.md) | Future reconciliation E2E catalog for v0.4 |
| [`roadmap.md`](roadmap.md) | Consolidated v0.1 → v0.8 roadmap; project evolution contract |

## Operational Records

| Document | Contents |
|---|---|
| [`iterations/current-iteration.md`](iterations/current-iteration.md) | Active v0.4 reconciliation work tracker |
| [`iterations/next-iteration.md`](iterations/next-iteration.md) | v0.5 topology and internal-networking reminder; not an execution tracker |
| [`design/caddy-unmatched-host-fallback.md`](design/caddy-unmatched-host-fallback.md) | Caddy routing-fallback design implemented in v0.3.1 |
