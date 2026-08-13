# Current Iteration

**Status:** in progress

**Base:** `6ccd178` (`chore: remove redundant VPS validation scripts`)

**Approved design:** [`caddy-unmatched-host-fallback.md`](../design/caddy-unmatched-host-fallback.md)

## Iteration - v0.4 Routing and Reconciliation

Objective: establish the remaining public-routing contract before implementing
reconciliation.

## Checkpoints

- [ ] Add the generic Caddy unmatched-host fallback defined in
  `caddy-unmatched-host-fallback.md`.
- [ ] Implement v0.4 reconciliation from the approved reconciliation design.

## Scope and Non-goals

- v0.4 begins with the Caddy fallback checkpoint. Reconciliation follows only
  after that checkpoint is complete.
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
