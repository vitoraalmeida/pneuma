# Consolidated Pneuma Roadmap - v0.1 to v0.8

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

**Status:** in progress.

Pneuma will converge runtime and exposure materialization toward persisted intent
without selecting a new Release or creating a Deployment from drift.

- [ ] Desired versus observed state.
- [ ] Drift detection and unambiguous recovery.
- [ ] Interrupted Deployment recovery.
- [ ] Better restart/reboot convergence.
- [ ] Candidate and activation improvements.
- [ ] Deployment mutual exclusion per Application.
- [ ] Non-interactive CLI with structured output and exit codes.

Already implemented, not v0.4 work: CI validation and image publishing under the
full commit-SHA tag; restricted SSH deployment through `pneuma ci dispatch`.

Out of scope until demonstrated need: registry watchers, automatic deployment
policies, complete audit logging, generic idempotency keys, image retention, and
automatic rollback after promotion.

## v0.5 - Application Topology and Internal Networking

- [ ] Service relationships and Application dependencies.
- [ ] Internal services and network/service addressing.
- [ ] System as a functional grouping mechanism.
- [ ] Basic service discovery.

## v0.6 - Network Policy Enforcement

- [ ] `pneuma-netd` host connectivity enforcement using nftables, default deny,
  and explicit connectivity.

## v0.7 - Workload Identity and Secure S2S

- [ ] SPIFFE and SPIRE workload identity per RuntimeInstance.
- [ ] `pneuma-proxy` for mTLS, authentication, authorization, and telemetry.

## v0.8 - Artifact Security and Secrets

- [ ] SBOM generation and enforcement.
- [ ] Image signature verification.
- [ ] Admission policies for unsigned artifacts.
- [ ] Secret management, injection, and rotation.
- [ ] Implemented threat model.

## Out of Scope Beyond v0.8

HTTP API, webhooks, centralized observability, multiple hosts, scheduler, remote
agents, distributed reconciliation, managed builds, canary or gradual
rollout, autoscaling, Kubernetes, RBAC, and multi-user support remain out of
scope until a future version explicitly revisits them.
