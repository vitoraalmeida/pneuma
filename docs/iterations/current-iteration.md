# Current Iteration

**Status:** in progress

**Base:** `95d86ab` (`feat: add Caddy unmatched-host fallback`)

**Approved design:** [`reconciliation.md`](../design/reconciliation.md)

## Iteration - v0.4 Reconciliation

Objective: converge runtime and exposure materialization toward persisted intent
without selecting a new Release or making destructive changes from ambiguity.

## Checkpoints

- [ ] Implement v0.4 reconciliation from the approved reconciliation design.

## Scope and Non-goals

- DNS, certificate lifecycle, registry watching, auto-deploy, API, TUI, OIDC,
  RBAC, multiple hosts, and precompiled binary download are out of scope.

## Acceptance Criteria

- Reconciliation converges runtime and exposure materialization only when the
  identity and desired state are unambiguous.
- Reconciliation does not create a Release or Deployment, change intent, or
  destructively repair ambiguous drift.
- Required VM E2E coverage proves the approved reconciliation scenarios.

## Blockers

None.
