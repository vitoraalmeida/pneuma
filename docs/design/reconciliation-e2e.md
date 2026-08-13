# E2E Catalog — Reconciliation v0.4

**Status:** catalog approved for future implementation; it does not describe
tests already executed or introduce `pneuma reconcile`.

**Semantics:** [`reconciliation.md`](reconciliation.md) defines authorities,
invariants, results, and non-goals. This catalog defines the operational proof
on a Debian 13 VM after the command exists.

## Boundaries

- These scenarios do not enter `scripts/dev-vm/test-all.sh` before
  `pneuma reconcile` exists.
- The catalog does not authorize creating a Release, building an image, querying
  a registry to choose a version, creating a Deployment because of drift,
  changing intent, or destructively repairing ambiguous identity.
- Each scenario observes SQLite, Podman/systemd, and Caddy as applicable. A
  successful process result without confirmed materialized state is not scenario success.

## Future Environment

Tests must use a clean Debian 13 VM with rootless Podman, `pneuma` user linger,
systemd user/Quadlet, Caddy, SQLite database, and fixture images available. The
harness must be able to:

- import by Git URL and deploy internal and public applications;
- inspect `runtime_instances`, `deployments`, and `exposures` in SQLite;
- remove/inspect containers and Quadlet units without changing identities;
- edit/remove Caddy fragments and confirm reload/external health;
- interrupt the Pneuma process at controlled points in non-terminal states;
- run commands with timeouts, collect logs, and clean resources at the end.

Each case must record fixture version, Application/Deployment/RuntimeInstance
IDs, initial desired state, and final result. Expected results use: `no-op`,
`repaired`, `deferred`, `failed`, `diverged`, or
`manual-intervention`.

## Runtime Drift

| Scenario | Injection and action | Expected result | Forbidden |
|---|---|---|---|
| Container removed before observation | Remove the active deployment container with `Running` intent; run reconcile. | With unit present and identity confirmed, `repaired`: the same RuntimeInstance/Deployment is reused, persisted port is retained, and `external_runtime_id` is reconciled by CAS. | Create Deployment/RuntimeInstance, tombstone `removed_at`, change port. |
| Container removed after `status` | Run `app status` to record `Missing`, remove/confirm absence, and run reconcile. | Same recovery as the prior case; `Missing` does not prevent recovery of logical identity. | Interpret `Missing` as retirement. |
| Unit present, container absent | Retain expected Quadlet and remove only the container. | Start the unit, observe the container by stable name, and confirm identity, endpoint, and CAS. | Rewrite the unit unnecessarily. |
| Unit absent, container absent | Remove expected Quadlet and container while preserving SQLite. | Rematerialize the same unit for the same RuntimeInstance and start only after confirming logical identity. | Create a new Deployment, allocate a new port, or promote candidate. |
| Divergent identity in Running runtime | Separately change digest, label, port, container, or Quadlet content/unit. | `manual-intervention` with diagnostics for the divergent field; no destructive effect. | Automatic stop/remove/replacement. |
| Reboot with Running intent | Reboot after correct materialization. | Active runtime returns and reconcile produces `no-op` or `repaired` only for renewed external ID. | Create a new Deployment. |
| Reboot with Stopped intent | Stop application, reboot, and run reconcile. | `no-op`; RuntimeInstance remains without `removed_at`, and an absent container is acceptable. | Start unit or tombstone runtime. |

## Exposure Drift

| Scenario | Injection and action | Expected result | Forbidden |
|---|---|---|---|
| Public fragment removed | Delete canonical fragment of a healthy `Public` Application. | `repaired`: recreate fragment, validate it, and confirm reload and external health; `active` state. | Change visibility or Deployment. |
| Divergent upstream | Change loopback target in the public fragment. | Replace with canonical contents, reload, and run external health; update `configuration_version` only after confirmation. | Preserve divergent fragment as success. |
| Correct fragment without confirmed reload | Leave correct content on disk and force reload to fail. | `Public` intent preserved; `failed` or `diverged` with diagnostics based on compensation; do not report active route. | Declare `no-op` based only on disk content. |
| Public intent without route | Persist `Public` with healthy runtime and missing fragment. | Materialize and confirm route, or recoverable `failed` if precondition/effect fails. | Revert intent to `Internal`. |
| Internal intent with route | Persist `Internal` and retain fragment on disk. | Remove fragment, reload, and confirm absence; `not_materialized` state. | Retain public route or change runtime. |
| Exposure compensation fails | Force failure after materialization/removal and restoration failure. | `diverged` with explicit diagnostics and observable state retained for intervention. | Assert complete compensation. |

## Interrupted Deployments

All cases preserve the prior healthy runtime and route. The harness interrupts
the process after the persisted transition and before the next effect, then runs
reconcile once.

| State | Minimum evidence and expected result |
|---|---|
| `Pending` | No confirmed external effect. Record failure/interruption and release only the proven associated reservation. |
| `Starting` | Observe candidate, unit, and container. Clean only resources with confirmed identity; record failure/interruption. |
| `Verifying` | Do not promote candidate without proven health. Clean confirmed candidate and preserve prior runtime/route. |
| `Activating` | Do not infer promotion from isolated fragment, reload, or runtime. Restore only what has safe compensation; record `diverged` if a known state cannot be asserted. |

In all cases: reconcile does not promote an ambiguous candidate, create a
Release, or change runtime intent or visibility.

## Concurrency and Idempotence

| Scenario | Execution | Expected result |
|---|---|---|
| Repeated reconcile | Run reconcile twice on correct materialization and, separately, on recoverable drift. | The second result is `no-op` after convergence; it duplicates no units, routes, RuntimeInstances, or Deployments. |
| Parallel reconcile | Start two reconciles for the same Application, blocking the first after it acquires reservation. | One converges or finishes; the other returns `deferred`, without `database is locked` or concurrent effects. |
| Deploy x deploy | Block deploy A after persisting a non-terminal Deployment and start B. | B returns `ActiveDeployment`; A may finish with one succeeded Deployment and one running runtime. The current CLI proof must remain as a regression. |
| Deploy x reconcile | Block deployment after persisting non-terminal state and run reconcile. | Reconcile returns `deferred` and does not touch candidate, prior runtime, Caddy, or cleanup. |

## Subsequent Automation

When `pneuma reconcile` exists, every row in this catalog becomes a named case
in the VM harness. The script must report PASS/FAIL/SKIP, retain logs, and never
mark an unavailable registry, network, VM, or credential dependency as PASS.
Skips require an explicit reason in the iteration tracker.
