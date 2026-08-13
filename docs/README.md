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
| [`design/reconciliation.md`](design/reconciliation.md) | Approved reconciliation semantics for v0.3 |
| [`design/reconciliation-e2e.md`](design/reconciliation-e2e.md) | Future reconciliation E2E catalog for v0.3 |
| [`roadmap.md`](roadmap.md) | Consolidated v0.1 → v0.7 roadmap; project evolution contract |

## Operational Records

| Document | Contents |
|---|---|
| [`operations/backup-and-restore.md`](operations/backup-and-restore.md) | Consistent SQLite database backup and recovery |
| [`iterations/current-iteration.md`](iterations/current-iteration.md) | Concluded bootstrap, VM, and E2E hardening iteration |
| [`design/application-import-store-extraction.md`](design/application-import-store-extraction.md) | Historical persistence-extraction design implemented during v0.2 consolidation |
