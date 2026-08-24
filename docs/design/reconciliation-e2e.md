# E2E Catalog — Reconciliation v0.4

**Status:** historical approved catalog; `pneuma reconcile` and the disposable-VM
harness `scripts/dev-vm/reconciliation-e2e.sh` exist. Runtime behavior is
described by [`../architecture/architecture.md`](../architecture/architecture.md);
the conservative decision deviations recorded in
[`reconciliation.md`](reconciliation.md) apply to the expected results below.

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

## Executable Mapping

Every scenario below is a named case in the future disposable-VM script
`scripts/dev-vm/reconciliation-e2e.sh`. Focused Rust tests supplement VM proof:
they exercise classification, compare-and-set, compensation, and fault injection
without replacing the VM case. Each VM case starts from one deployed fixture
baseline, records the fixture digest and Application, Deployment, and
RuntimeInstance IDs, captures SQLite rows and external command logs before and
after the action, and removes its resources during cleanup.

| ID | VM case | Focused Rust coverage | Fixture and injection | Required assertions |
|---|---|---|---|---|
| R1 | `runtime-container-removed-before-observation` | Missing runtime repair and external-ID CAS | Internal `healthy-http`; remove active container before reconcile. | Same logical IDs and port; only confirmed external ID may change; no tombstone or new logical row. |
| R2 | `runtime-container-removed-after-status` | Missing observation is not retirement | Internal `healthy-http`; run status, remove container, then reconcile. | Missing remains non-retiring; recovery preserves RuntimeInstance and endpoint. |
| R3 | `runtime-unit-present-container-absent` | Existing-unit recovery | Internal `healthy-http`; retain canonical Quadlet, remove container. | Unit bytes unchanged; one canonical container, digest, labels, endpoint, and loopback health confirmed. |
| R4 | `runtime-unit-and-container-absent` | Missing-unit rematerialization | Internal `healthy-http`; remove container and expected unit, preserving SQLite. | Same Deployment, RuntimeInstance, and port; canonical unit is recreated and healthy. |
| R5 | `runtime-divergent-identity` | Parameterized digest, label, port, name, and unit drift | Internal fixture with each external identity field independently divergent. | `manual-intervention`; no start, stop, removal, unit write, port allocation, or logical-row change. |
| R6 | `runtime-reboot-running` | Recreated-ID classification | Internal `healthy-http` with Running intent; reboot. | User manager, unit, container, digest, labels, port, and health are correct; result is `no-op` or renewed-ID `repaired`. |
| R7 | `runtime-reboot-stopped` | Stopped/missing no-op | Stop internal fixture, reboot, then reconcile. | Runtime remains non-retired; no start occurs; result is `no-op`. |
| E1 | `exposure-public-fragment-removed` | Public fragment recreation | Public `redirect-public`; delete canonical fragment. | Same runtime and deployment; canonical fragment, validation, reload, and external health produce active exposure. |
| E2 | `exposure-divergent-upstream` | Divergent fragment replacement | Public fixture; replace loopback upstream and reload it. | Canonical bytes replace divergent bytes; configuration version advances only after reload and health. |
| E3 | `exposure-correct-fragment-reload-unconfirmed` | Ordered reload failure and compensation | Public fixture; keep canonical bytes and force reload failure. | Public intent remains; no active route is claimed; result is `failed` or `diverged` with diagnostics. |
| E4 | `exposure-public-intent-without-route` | Public materialization | Healthy public-intent fixture with no fragment or route evidence. | Intent remains Public; route is materialized and confirmed or an explicit recoverable failure is stored. |
| E5 | `exposure-internal-intent-with-route` | Internal route removal | Persist Internal intent while retaining the fragment. | Fragment is removed and reload confirmed; runtime remains healthy; route becomes not materialized. |
| E6 | `exposure-compensation-fails` | Failed restoration classification | Public fixture; fail after materialization and fail restoration. | Observable final state and both diagnostics remain; result is `diverged`. |
| I1 | `interrupted-pending` | Pending interruption recovery | Interrupt after Pending persistence and before an external effect. | No candidate effect is assumed; prior runtime and route remain active; only proven reservation is released. |
| I2 | `interrupted-starting` | Confirmed-candidate cleanup | Interrupt after a candidate unit/container is identifiable. | Only candidate resources with confirmed identity are cleaned; prior runtime and route remain active. |
| I3 | `interrupted-verifying` | No ambiguous promotion | Interrupt while candidate health is unproven. | Candidate is never promoted; confirmed candidate cleanup preserves prior active materialization. |
| I4 | `interrupted-activating` | Route compensation and divergence | Interrupt after candidate activation begins. | Prior route is restored only when provable; otherwise result is `diverged`; candidate is never inferred active. |
| C1 | `concurrency-repeated-reconcile` | Repeated no-op and repaired convergence | Run twice on correct materialization and recoverable drift. | Second result is `no-op`; no duplicate unit, route, RuntimeInstance, Deployment, or port. |
| C2 | `concurrency-parallel-reconcile` | Reservation and stale-result faults | Block first reconcile after reservation and start a second. | One converges, one defers; no database lock or concurrent external effect. |
| C3 | `concurrency-deploy-deploy` | Existing CLI deployment gate regression | Block deploy A after a non-terminal transition and invoke deploy B. | B reports active deployment; A remains the only successful activation path. |
| C4 | `concurrency-deploy-reconcile` | Deferred-before-observation | Block a deployment in a non-terminal state and invoke reconcile. | Reconcile is `deferred` before Podman, systemd, Caddy, curl, or cleanup work. |

The VM harness must expose deterministic process gates for `Pending`,
`Starting`, `Verifying`, and `Activating`; polling and killing an arbitrary
process state is not sufficient evidence. It must retain a case-specific log
directory containing command output, SQLite dumps for `runtime_instances`,
`deployments`, and `exposures`, and pre/post external observations.
