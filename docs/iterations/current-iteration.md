# Current Iteration

**Status:** in progress

**Base:** `6ccd178` (`chore: remove redundant VPS validation scripts`)

**Approved design:** [`caddy-unmatched-host-fallback.md`](../design/caddy-unmatched-host-fallback.md)

## Iteration - v0.4 Routing and Reconciliation

Objective: establish the remaining public-routing contract before implementing
reconciliation.

## Checkpoints

- [x] Add the generic Caddy unmatched-host fallback defined in
  `caddy-unmatched-host-fallback.md`.
  Result: the shared production/VM Caddy baseline returns `Not Found` with HTTP
  404 for unmatched hosts, while public fragments retain domain routing. Clean
  bootstrap/rerun on a disposable Debian 13 clone passed 89 PASS/0 FAIL; full
  VM E2E passed 45 PASS/0 FAIL/0 SKIP, including public-to-internal fallback and
  a running internal runtime. Both disposable clones were destroyed.
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
