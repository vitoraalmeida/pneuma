# Current Iteration

**Status:** em andamento

**Base:** `d643a32` (`chore(release): v0.4.0`)

**Approved design:** [`domain-hardening-sweep.md`](../design/domain-hardening-sweep.md)

## Iteration - v0.4.1 Domain Hardening Sweep

Objective: strengthen the domain boundary by moving remaining business rules,
vocabulary, and typed identities into their owning domain modules, eliminating
primitive round-trips, and removing duplicate or drifted validation.

## Checkpoints

- [x] Centralize the loopback endpoint invariant and replace raw container-port
  fields with `ContainerPort`. Make runtime registration accept
  `ExpectedRuntimeEndpoint` and remove the hardcoded loopback address from
  port-reservation queries.
  Result: `RuntimeInstance`/`RuntimeRegistration` now carry `ContainerPort`;
  `register_candidate_runtime` and `port_is_reserved` accept
  `ExpectedRuntimeEndpoint`; the IPv6 `::1` drift in internal health checks is
  fixed; Caddy and local runtime delegate loopback validation to the domain.
- [ ] Pass `&HealthCheckSpecification` into the internal health-check adapter and
  fix the hardcoded `/` `200` probe used during visibility changes so it uses
  the persisted runtime health specification.
  Result: TBD.
- [ ] Move `DeploymentTransition`, its edge mapping, and promotion eligibility
  into `domain/deployment.rs`; move promotion/rollback target types out of
  `deployment_store`; merge duplicate promoted-candidate types.
  Result: TBD.
- [ ] Move public exposure target and outcome types into `domain/exposure.rs`;
  route public exposure through `ExposureIntent::new`; type `change_exposure`
  and helpers with `&ApplicationId`.
  Result: TBD.
- [ ] Introduce a domain function for stable container/unit naming; move
  external container identity format rules into the domain; type
  `runtime_store` external-id functions with `&ContainerId`.
  Result: TBD.
- [ ] Type adapter and store boundaries: Caddy fragment APIs, `ApplicationLock`,
  `operation_store`, `port_allocator`, `oci_image`, internal use-case input
  structs, and `reconcile_application` with typed identities.
  Result: TBD.
- [ ] Align CI dispatch name validation with `ApplicationName::new`; share the
  catalog-name validator; move `DeliveryType`/`DeliverySpecification` to their
  canonical domain modules.
  Result: TBD.

## Scope and Non-goals

- No new feature, CLI command, schema migration, or persisted representation
  change.
- No new dependency, async code, trait abstraction, or generic boundary.
- No reconciliation, networking, topology, or v0.5 work.
- The only intentional behavior changes are the two approved fixes: loopback
  health checks reject IPv6 `::1`, and visibility changes use the persisted
  health path/status.

## Acceptance Criteria

- Loopback endpoint validation has a single domain owner.
- Health-check configuration crosses adapter boundaries as a typed value.
- Deployment lifecycle, promotion eligibility, and target types live in the
  domain.
- Exposure intent and public-exposure targets live in the domain.
- Container/unit naming and external identity format rules live in the domain.
- Adapter and store APIs accept typed identities instead of primitive strings.
- CI dispatch validates application names with the same rule as the domain.
- The exact CI gates and VM regression are green before closure.

## Closure Evidence

- TBD.

## Blockers

- None.
