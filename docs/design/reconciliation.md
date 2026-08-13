# Design — Reconciliation

**Status:** approved design for v0.4; it does not describe behavior already
implemented. It is queued for the next iteration; execution and progress will
live in `docs/iterations/current-iteration.md` when that iteration is promoted.

## Objective

Define the semantics that future `pneuma reconcile` will use to converge an
Application to persisted intent, without selecting a new version or turning an
ambiguous observation into a destructive change. This design guides the domain,
runtime, and exposure refactors completed in v0.3.

This document does not introduce the command, APIs, migrations, or retry policies.

## Model and Vocabulary

- **Intent:** desired state persisted in SQLite, including runtime and
  visibility.
- **Logical state:** persisted Application, Release, Deployment, and
  RuntimeInstance; their IDs are not replaced with external IDs.
- **Observed state:** container and Quadlet unit observed in Podman/systemd, and
  route observed in Caddy.
- **Correct materialization:** observed state that fully corresponds to intent
  and expected logical identity.
- **Drift:** divergence between observed materialization and expected
  configuration or identity.
- **Retirement:** intentional removal of a runtime during cleanup or active
  runtime replacement. Only retirement writes `removed_at`.

Release is the immutable OCI artifact. Deployment is the attempt to activate a
Release. RuntimeInstance is the concrete materialization of a Deployment and is
reused during recoverable drift recovery.

## Sources of Truth

| System | Authority |
|---|---|
| SQLite | Intent, history, and logical identities. |
| Podman/systemd | Observed state of the container and Quadlet unit. |
| Caddy | Materialized fragment, reload, and route. |
| OCI registry | Artifact availability, never version selection by reconcile. |

The reconciler does not query the registry to create or select a Release. An
Application with a missing container does not receive a new Deployment for that reason.

## Invariants

1. The active deployment and its active runtime define the logical identity that
   can be recovered. Recovery creates no new Deployment or RuntimeInstance.
2. `Missing` is an observation, not a tombstone. `removed_at` is reserved for
   candidate cleanup, prior-runtime retirement, and intentional removal.
3. `Running/Running` is a no-op only when active deployment, image digest,
   labels, loopback endpoint, port, Quadlet unit, and container correspond to
   the expected materialization.
4. No SQLite transaction remains open during Podman, systemd, Caddy, or HTTP.
   The use case observes or materializes externally and persists the result in a
   short transaction.
5. All persistence after an external effect uses compare-and-set and row count.
   Zero rows is stale or concurrent state, never success.
6. v0.2 materializations that use `release.id` in labels, Quadlet, or
   `configuration_version` remain observable until redeployment or
   rematerialization. New materializations use the identity rules defined in
   the corresponding refactors.
7. Given ambiguous identity or divergent configuration, reconcile does not stop,
   remove, replace, or promote resources automatically without an explicit policy.

## Runtime Reconciliation

Before acting, reconcile loads intent, the active deployment, and the successful
non-removed RuntimeInstance. Observation confirms the deterministic container/unit
name, its relationship to the Application and Deployment, image digest, labels,
loopback port, and Quadlet unit content/configuration. Updating
`external_runtime_id` uses the expected logical runtime and fails as
`RuntimeChanged` if the CAS does not update exactly one row.

| Desired | Observed | Action |
|---|---|---|
| Running | Running, correct identity and configuration | No-op. |
| Running | Stopped, unit present and identity confirmed | Start the unit and observe again. |
| Running | Missing, unit present and identity confirmed | Start the unit, resolve the container by stable name, and reconcile `external_runtime_id`. |
| Running | Missing, unit absent and logical identity confirmed | Rematerialize the same unit for the same RuntimeInstance, start it, and reconcile `external_runtime_id`. |
| Running | Failed | Collect safe diagnostics; restart only when unit and identity are confirmed and recovery policy permits it. Otherwise, report without destructive change. |
| Running | Running with divergent digest, label, port, container, or unit | Report drift and require explicit policy or manual intervention. |
| Stopped | Running, Starting, Created, or Stopping | Stop the unit when it is the expected unit; observe again. |
| Stopped | Stopped | No-op. |
| Stopped | Missing | No-op; retain RuntimeInstance and do not write `removed_at`. |
| Stopped | Failed or divergent unit | Report diagnostics; do not remove the unit or tombstone the runtime. |

On rematerialization, the persisted RuntimeInstance port is the expected endpoint
identity. If it cannot be safely used, reconcile reports drift instead of silently
allocating another port. The legacy runtime remains operable through v0.2
compatibility until a rematerialization applies the new identity.

## Exposure Reconciliation

Visibility is persisted intent. Public exposure is correct only if the expected
canonical fragment is on disk, reload has been confirmed, and external route
health has passed. A correct fragment without confirmed reload or external health
is not a correct route.

`configuration_version` identifies the canonical representation of the Caddy
fragment (`domain` and endpoint). It does not identify Release, Deployment, or RuntimeInstance.

| Desired | Observed | Action |
|---|---|---|
| Public | Canonical fragment, confirmed reload, and correct external health | No-op. |
| Public | Missing fragment, healthy active runtime, and confirmed endpoint | Materialize, reload, and run external health. |
| Public | Divergent fragment, healthy active runtime, and confirmed endpoint | Replace with canonical fragment, reload, and run external health. |
| Public | No confirmed healthy active runtime, domain, or endpoint | Record recoverable diagnostics; do not publish a route. |
| Internal | Missing fragment | No-op. |
| Internal | Present fragment | Remove, reload, and confirm removal. |

Failure during materialization, reload, or health preserves requested intent and
writes `failed` with diagnostics. If compensation cannot restore a known observable
situation, it writes `diverged`. Reconcile may repair `failed` when the expected
fragment and runtime identity are unambiguous; `diverged` is reported for manual
intervention until an explicit policy defines how to replace an ambiguous-origin fragment.

## Interrupted Deployment Recovery

A Deployment in `Pending`, `Starting`, `Verifying`, or `Activating` reserves the
Application and prevents reconcile from competing for its effects. Future recovery
first observes candidate, unit, route, and prior runtime, preserving the already
active healthy version.

| Interrupted status | Recovery rule |
|---|---|
| Pending | No confirmed external effects: record failure/interruption and release only resources proven associated with the deployment. |
| Starting | Observe candidate and unit; clean only the proven associated candidate, record failure/interruption, and preserve the prior runtime. |
| Verifying | Do not promote a candidate with unproven health; clean a candidate with confirmed identity, record failure/interruption, and preserve prior runtime/route. |
| Activating | Do not infer promotion from an isolated fragment, reload, or runtime. If atomic promotion is not proven, restore only what has safe compensation, record failure/interruption, and mark exposure `diverged` when recovery is incomplete. |

Recovery never automatically promotes an ambiguous candidate. Cleanup failures
are recoverable diagnostics and do not revoke promotion already atomically confirmed.

## Concurrency and Results

This design creates no additional lock. Existing deployment retains the logical
reservation through a non-terminal Deployment. While a non-terminal Deployment
exists, reconcile returns `deferred` with the deployment blocking the operation
and does not trigger concurrent runtime, Caddy, or cleanup work.

The future command must serialize `reconcile × reconcile` per Application through
the same reservation or an equivalent persisted primitive before executing external
effects. A lost CAS after an external effect results in `failed` or `deferred`,
with diagnostics and without reporting success.

Observable results are:

- `no-op`: materialization was already correct;
- `repaired`: recoverable divergence was converged and confirmed;
- `deferred`: a deployment or concurrent change prevents safe action;
- `failed`: convergence was not completed, with recoverable diagnostics;
- `diverged`: compensation or observation does not allow materialization to be asserted;
- `manual-intervention`: identity or configuration drift requires an explicit
  policy.

## Non-goals

Reconcile does not create a Release, build, discover a new artifact, select a
registry version, create a Deployment because a container died, change desired
runtime state or visibility, promote an uncertain candidate, or perform destructive
repair given ambiguous identity.

## Future Acceptance Scenarios

The subsequent E2E catalog must cover at minimum:

- removed container with unit present and with unit absent;
- divergent digest, label, port, container, or unit;
- reboot and stopped/running runtime recovery;
- missing or divergent-target Caddy fragment, unconfirmed reload, and desired
  visibility incompatible with the route;
- interruption at each non-terminal Deployment status;
- repeated reconcile, parallel reconcile, deploy × deploy, and deploy × reconcile.
