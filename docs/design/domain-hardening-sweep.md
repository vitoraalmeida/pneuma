# Design - Domain Hardening Sweep

**Status:** historical record. Approved as v0.4.1 and fully implemented
(shipped within the v0.4.2 release); implemented behavior lives in
[`../architecture/`](../architecture/). Execution history is in Git.

## Objective

Strengthen the domain boundary by moving remaining business rules, vocabulary,
and typed identities into their owning domain modules, eliminating primitive
round-trips at use-case and adapter boundaries, and removing duplicate or
drifted validation.

## Fixed Decisions

- The IPv4 loopback runtime-endpoint invariant (`127.0.0.1`, non-zero port)
  belongs exclusively to `domain/runtime.rs` and is carried by
  `ExpectedRuntimeEndpoint`. Adapters and use cases accept the typed endpoint
  instead of `SocketAddr` and do not re-implement the check.
- `ContainerPort` is the canonical representation of the container-facing port
  inside `RuntimeInstance`, `RuntimeRegistration`, and store hydration. Raw
  `u16` fields are replaced by the validated newtype.
- Health-check configuration (path, expected status, endpoint) crosses adapter
  boundaries as `&HealthCheckSpecification`; health-check adapters do not
  revalidate path, status, or loopback rules.
- Exposure intent validation lives in `domain/exposure.rs` (`ExposureIntent::new`).
  Use cases that need a public exposure target pass a validated `ExposureIntent`
  or `PublicExposureTarget` instead of raw `(Visibility, Option<DomainName>)`.
- Deployment lifecycle transitions and eligibility are domain rules owned by
  `domain/deployment.rs`. `DeploymentTransition`, its edge mapping, and the
  promotion-candidate predicate are methods/functions in that module.
- Promotion and rollback target types (`PromotionTarget`, `RollbackTarget`) and
  promotion result types (`PromotedCandidate`) belong to `domain/deployment.rs`,
  not to `deployment_store` or individual use cases.
- Stable container/unit naming (`pneuma-{application}-{deployment}`) is a pure
  domain function over `ApplicationName` and `DeploymentId` owned by
  `domain/runtime.rs`; Podman and Quadlet adapters consume it.
- External container identity format rules belong to `domain/runtime.rs` (on
  `ContainerId` or a dedicated newtype). `ApplicationId` format ownership is
  clarified and enforced once, at the domain boundary.
- Typed logical identities (`ApplicationId`, `DeploymentId`, `RuntimeInstanceId`,
  `ApplicationName`, `SystemName`, `DomainName`, `OciRepository`) cross use-case
  and store APIs as their newtype forms; conversion to `&str`/text happens only
  at the SQLite parameter boundary.
- CI dispatch validates application names with the same rule as the domain
  (`ApplicationName::new`), rejecting names that would later be reported as
  "not found".
- `DeliveryType` and `DeliverySpecification` move to their canonical owners:
  `DeliveryType` to `manifest.rs` (it is a manifest value), and
  `DeliverySpecification` to `release.rs` (it wraps `OciRepository`).
- This iteration preserves SQLite representation, CLI commands, persisted text,
  migration history, and external-effect ordering. The only intentional behavior
  changes are the two approved fixes: loopback health checks no longer accept
  IPv6 `::1`, and visibility changes probe the application's persisted health
  path/status instead of hardcoded `/` and `200`.

## Checkpoint Order

1. **Loopback endpoint and port types** — centralize the loopback invariant in
   `domain/runtime.rs`; replace `RuntimeInstance`/`RuntimeRegistration` raw
   `u16` container-port fields with `ContainerPort`; make
   `register_candidate_runtime` accept `ExpectedRuntimeEndpoint`; remove the
   hardcoded `"127.0.0.1"` from `runtime_store::port_is_reserved`.
2. **Health-check contract and exposure behavior fix** — pass
   `&HealthCheckSpecification` into the internal health-check adapter; fix the
   hardcoded `/` `200` probe in `exposure_change.rs` to use the persisted
   runtime health specification; correct the error mapping in
   `exposure_change.rs`.
3. **Deployment lifecycle and promotion domain consolidation** — move
   `DeploymentTransition` and its edge mapping into `domain/deployment.rs`; add
   a single promotion-candidate predicate; move `PromotionTarget` and
   `RollbackTarget` from `deployment_store` to `domain/deployment.rs`; merge
   `PromotedCandidate`/`PromotedPublicCandidate`; move `completed_promotion`
   onto the target type.
4. **Exposure domain consolidation** — move `PublicExposureTarget` and
   `ExposureOutcome` to `domain/exposure.rs`; route public-exposure checks
   through `ExposureIntent::new`; change `change_exposure` and its helpers to
   accept `&ApplicationId`.
5. **External runtime identity and stable naming** — introduce a domain function
   for stable container/unit naming; move container-id format rules into the
   domain; make `runtime_store` external-id functions accept `&ContainerId`;
   remove duplicated `is_container_id` checks.
6. **Typed adapter and store boundaries** — type Caddy adapter parameters
   (`ApplicationId`, `DomainName`, `ExpectedRuntimeEndpoint`); type
   `ApplicationLock`, `operation_store`, `port_allocator`, and `oci_image` APIs;
   type internal use-case input structs; make `reconcile_application` accept
   `&ApplicationName`.
7. **CI dispatcher alignment and final cleanup** — make CI dispatch validate
   application names through `ApplicationName::new`; share the catalog-name
   validator between `ApplicationName` and `SystemName`; move
   `DeliveryType`/`DeliverySpecification`; convert `health_failure_message` to
   a `Display` impl on the failure type.

## Validation

- Focused tests cover corrected loopback and health-check behavior, typed
  identity round-trips, and moved domain rules.
- CLI and integration coverage confirms no command output, SQLite
  representation, migration behavior, or external-effect ordering changes
  beyond the two approved behavior fixes.
- Every checkpoint passes `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`.
- Iteration closure runs the full VM regression suite (`test-all.sh` and
  `reconciliation-e2e.sh`) on a disposable clone.

## Non-goals

- No new feature, CLI command, schema migration, or persisted representation
  change.
- No new dependency, async code, trait abstraction, or generic boundary.
- No reconciliation, networking, topology, or v0.5 work.
