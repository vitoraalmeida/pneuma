# Current Iteration

**Status:** in progress

**Base:** `49b9476` (`docs: refactor documentation architecture`)

**Approved designs:** [`domain-model-hardening.md`](../design/domain-model-hardening.md),
[`reconciliation.md`](../design/reconciliation.md), and [`tui.md`](../design/tui.md)

## Iteration - v0.4 Reconciliation

Objective: converge runtime and exposure materialization toward persisted intent
without selecting a new Release or making destructive changes from ambiguity.

## Checkpoints

- [x] Complete the Application domain projection with persisted runtime intent
  and specification version in every Application read path. This establishes
  explicit Application intent before the broader reconciliation input model.
  Result: import, listing, and System details now hydrate typed runtime intent
  and specification version through one store-owned row mapper.
- [x] Separate the core Application from catalog summaries and represent source,
  delivery, runtime, and health configuration with named domain projections.
  Replace scalar and tuple specification loaders and remove direct SQL from
  deployment use cases.
  Result: command lookup loads a core Application directly; catalog output and
  deployment configuration use named typed projections mapped by the store.
- [x] Establish Exposure as a domain concept with typed visibility and
  materialization state, keeping manifest parsing as an input boundary and
  mapping persisted values in the store.
  Result: Exposure now carries persisted intent, route state, runtime identity,
  configuration version, and diagnostics; the store rejects invalid state text.
- [x] Establish RuntimeInstance and runtime observation as domain concepts,
  replace runtime tuples and use-case-local runtime entities, and hydrate the
  persisted observed state instead of inventing `Running` on reload.
  Result: runtime registration, lifecycle, cleanup, and exposure share typed
  RuntimeInstance and observation models mapped by the runtime store.
- [x] Represent immutable OCI artifact identity as one validated value during
  Release creation, and remove invented identifiers from Release error mapping.
  Result: Release owns a validated OciArtifact, stores reject inconsistent
  persisted artifact parts, and errors preserve real identifiers and sources.
- [x] Add concise reading comments to production structs and functions before
  reconciliation work. Explain responsibility, relevant constraints, and
  non-obvious mechanisms without changing behavior or annotating tests.
  Result: production structs and operational functions now describe their role,
  transaction or external-effect boundaries, and critical invariants.
- [x] Harden domain identities across logical and external resources without
  changing SQLite's persisted text representation.
  Result: opaque logical and container identities now map through stores without
  changing SQLite TEXT values.
- [x] Validate application specification and OCI values at domain boundaries,
  including shared repository identity and cohesive source representation.
  Result: validated specification values and cohesive sources now reject invalid
  input and persisted malformed text maps to contextual store errors.
- [x] Separate expected runtime identity from external observation, preserve
  `Missing`, and model retirement explicitly.
  Result: expected loopback endpoints remain reserved identity, observations
  preserve Missing and drift, status is read-only for container identity, and
  retirement derives from explicit removal evidence.
- [x] Make Exposure intent, confirmed route evidence, and diagnostics valid by
  construction while retaining compensation-relevant evidence.
  Result: typed exposure intent, route evidence, and diagnostics reject invalid
  persisted combinations while preserving confirmed routes through transitions.
- [x] Add Deployment lifecycle evidence and replace scalar or tuple operation
  results with cohesive domain values.
  Result: Deployment hydration now validates lifecycle evidence, preserves incomplete
  historical failures, exposes typed blockers and history, and uses named execution
  and rollback targets.
- [x] Move persisted-value conversion into stores and require explicit stale
  outcomes from compare-and-set persistence primitives.
  Result: stores own SQLite mapping and CAS writes return explicit updated or
  stale outcomes; promotion, catalog, and rollback use typed store values.
- [x] Convert `reconciliation-e2e.md` into an executable reconciliation test
  plan before implementation: assign every scenario to a focused Rust test or
  disposable-VM E2E case, define fixtures, fault injection, persisted-state and
  external-observation assertions, and add initial failing tests for the first
  implementation slice.
  Result: all approved scenarios have named VM cases and supplemental focused
  coverage, fixtures, injections, and persisted/external assertions; the initial
  reconciliation tests were demonstrated red before implementation.
- [x] Define reconciliation input and read-only observation: load persisted
  Application intent, active Deployment, RuntimeInstance, and Exposure; observe
  Podman/systemd and Caddy without changing SQLite or external resources.
  Result: a short SQLite snapshot now loads Application, blocker, active
  Deployment, Release, RuntimeInstance, Exposure, and specification before
  read-only container, Quadlet, and Caddy fragment observation.
- [x] Add `pneuma reconcile <application>` with observable `no-op` and
  `deferred` results. A non-terminal Deployment must defer reconciliation before
  any external effect.
  Result: the top-level command returns `deferred` before external observation
  for a non-terminal Deployment and reports `no-op` for stopped internal intent
  with absent runtime and route; runtime repair remains a later checkpoint.
- [x] Reconcile confirmed runtime drift: recover a missing container only when
  the persisted RuntimeInstance, deterministic unit/container identity, image
  digest, labels, and loopback endpoint are unambiguous. Preserve the persisted
  port and reconcile `external_runtime_id` by compare-and-set.
  Result: reconcile now CAS-updates only a confirmed recreated Quadlet container
  restarts an existing canonical Quadlet or rematerializes an absent one from the
  persisted runtime port;
  divergent runtime identity or configuration remains manual intervention.
- [x] Reconcile Caddy exposure drift: repair missing or divergent public
  fragments only with a healthy confirmed runtime; remove an internal route;
  validate, reload, externally health-check, and preserve `failed` or `diverged`
  diagnostics when convergence cannot be confirmed.
  Result: reconcile CAS-reserves exposure intent, repairs canonical public routes
  through configured Caddy validation/reload and external health, removes internal
  fragments after validation/reload, and records failed, diverged, or manual outcomes.
- [x] Handle interrupted Deployments and concurrency: clean only resources with
  proven identity, preserve the active healthy runtime and route, and serialize
  reconcile against reconcile and deployment effects per Application.
  Result: a free per-Application lock recovers interrupted deployments by stage;
  pending work fails without effects, proven candidates are cleaned, and uncertain
  activation routes are retained as diverged without promotion.
- [x] Complete the approved VM E2E catalog and final regression. Record actual
  PASS/FAIL/SKIP evidence for bootstrap and reconciliation scenarios; destroy
  every disposable clone.
  Result: `reconciliation-e2e.sh` passed all R1-R7, E1-E6, I1-I4, and C1-C4
  cases (21 PASS, 0 FAIL, 0 SKIP) in 383 seconds on 2026-08-20 after preparing
  shared fixture inputs once. The disposable clone was removed.
- [x] Complete runtime cleanup domain adoption: return typed lifecycle state from
  the store and preserve distinct RuntimeInstance and Container identities through
  candidate cleanup and compensation.
  Result: cleanup now receives typed lifecycle and distinct runtime/container IDs;
  malformed persisted runtime state is rejected by the runtime store.
- [x] Complete promotion domain adoption: replace raw `PromotionTarget` IDs,
  domain text, and retirement timestamp with validated domain values.
  Result: promotion targets now map SQLite values to typed identities, retirement
  evidence, and domains before the promotion use cases validate them.
- [ ] Preserve artifacts and runtime specifications through candidate orchestration
  until adapter boundaries, without independent reference/digest or health scalars.
- [ ] Replace remaining use-case tuple and raw identity operation outputs with
  cohesive typed values, and remove invented identifiers from store errors.
- [ ] Add TUI dashboard read projections for Systems, Applications, details,
  Deployments, Releases, RuntimeInstances, and read-only runtime observation.
- [ ] Extract shared remote import, diagnostics, database, and progress-enabled
  deployment orchestration from `main.rs` without changing CLI behavior.
- [ ] Add `pneuma tui` with Ratatui/Crossterm terminal safety, self-contained
  tabs, read-only navigation, and renderer/reducer coverage.
- [ ] Add confirmed forms and serialized background jobs for approved operator
  actions, then complete TUI terminal and fake-command regression evidence.

## Scope and Non-goals

- DNS, certificate lifecycle, registry watching, auto-deploy, API, OIDC,
  RBAC, multiple hosts, and precompiled binary download are out of scope.
- Reconciliation never creates a Release, selects a registry artifact, or
  changes desired runtime state or visibility.
- Domain hardening preserves persisted representation and never silently repairs
  ambiguous historical values.
- Ambiguous identity or configuration drift is reported for manual intervention,
  not repaired destructively.

## Acceptance Criteria

- Reconciliation converges runtime and exposure materialization only when the
  identity and desired state are unambiguous.
- Reconciliation does not create a Release or Deployment, change intent, or
  destructively repair ambiguous drift.
- Required VM E2E coverage proves the approved reconciliation scenarios.
- The TUI preserves non-interactive CLI behavior, does not expose CI dispatch,
  and verifies terminal restoration and approved operator workflows.
- The exact CI gates and all required VM evidence are green before this
  iteration is closed.

## Blockers

- None.
