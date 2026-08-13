# Current Iteration

**Status:** in progress

**Base:** `49b9476` (`docs: refactor documentation architecture`)

**Approved design:** [`reconciliation.md`](../design/reconciliation.md)

## Iteration - v0.4 Reconciliation

Objective: converge runtime and exposure materialization toward persisted intent
without selecting a new Release or making destructive changes from ambiguity.

## Checkpoints

- [ ] Convert `reconciliation-e2e.md` into an executable reconciliation test
  plan before implementation: assign every scenario to a focused Rust test or
  disposable-VM E2E case, define fixtures, fault injection, persisted-state and
  external-observation assertions, and add initial failing tests for the first
  implementation slice.
- [ ] Define reconciliation input and read-only observation: load persisted
  Application intent, active Deployment, RuntimeInstance, and Exposure; observe
  Podman/systemd and Caddy without changing SQLite or external resources.
- [ ] Add `pneuma reconcile <application>` with observable `no-op` and
  `deferred` results. A non-terminal Deployment must defer reconciliation before
  any external effect.
- [ ] Reconcile confirmed runtime drift: recover a missing container only when
  the persisted RuntimeInstance, deterministic unit/container identity, image
  digest, labels, and loopback endpoint are unambiguous. Preserve the persisted
  port and reconcile `external_runtime_id` by compare-and-set.
- [ ] Reconcile Caddy exposure drift: repair missing or divergent public
  fragments only with a healthy confirmed runtime; remove an internal route;
  validate, reload, externally health-check, and preserve `failed` or `diverged`
  diagnostics when convergence cannot be confirmed.
- [ ] Handle interrupted Deployments and concurrency: clean only resources with
  proven identity, preserve the active healthy runtime and route, and serialize
  reconcile against reconcile and deployment effects per Application.
- [ ] Complete the approved VM E2E catalog and final regression. Record actual
  PASS/FAIL/SKIP evidence for bootstrap and reconciliation scenarios; destroy
  every disposable clone.

## Scope and Non-goals

- DNS, certificate lifecycle, registry watching, auto-deploy, API, TUI, OIDC,
  RBAC, multiple hosts, and precompiled binary download are out of scope.
- Reconciliation never creates a Release, selects a registry artifact, or
  changes desired runtime state or visibility.
- Ambiguous identity or configuration drift is reported for manual intervention,
  not repaired destructively.

## Acceptance Criteria

- Reconciliation converges runtime and exposure materialization only when the
  identity and desired state are unambiguous.
- Reconciliation does not create a Release or Deployment, change intent, or
  destructively repair ambiguous drift.
- Required VM E2E coverage proves the approved reconciliation scenarios.
- The exact CI gates and all required VM evidence are green before this
  iteration is closed.

## Blockers

None.
