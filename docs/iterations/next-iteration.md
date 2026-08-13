# Next Iteration

**Status:** queued, not an execution tracker.

**Target:** v0.4 - Reconciliation and Deployment Reliability.

The documentation architecture refactor precedes v0.4. Do not implement this
queued work until the current iteration is closed and this tracker is promoted.

**Approved design:** [`../design/reconciliation.md`](../design/reconciliation.md)

## Objective

Converge runtime and exposure materialization toward persisted intent without
selecting a new Release or making destructive changes from ambiguity.

## Checkpoints

- Convert the reconciliation E2E catalog into an executable test plan.
- Add read-only observation and `pneuma reconcile <application>` results.
- Repair unambiguous runtime and Caddy drift.
- Handle interrupted Deployments and per-Application concurrency safely.
- Complete the approved disposable-VM E2E catalog and final regression.

## Boundaries

- Reconciliation does not create a Release or Deployment, select a registry
  artifact, or change desired runtime state or visibility.
- Ambiguous identity or configuration drift requires manual intervention.

See [`../design/reconciliation.md`](../design/reconciliation.md) for complete
semantics and [`../roadmap.md`](../roadmap.md) for later v0.5 work.
