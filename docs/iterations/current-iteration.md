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

- Unmatched HTTP requests receive the generic fallback without identifying an
  Application.
- Existing public routes continue to work; internalized Applications retain a
  running internal runtime.
- Bootstrap and VM E2E coverage prove the behavior.

## Blockers

None.
