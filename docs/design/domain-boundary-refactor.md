# Design - Domain Boundary Refactor

**Status:** approved design for v0.4 after reconciliation. It does not describe
implemented behavior. Execution and progress live only in
[`../iterations/current-iteration.md`](../iterations/current-iteration.md).

## Objective

Align domain modules, store ownership, and use-case transaction boundaries with
the concepts they represent without changing externally observable behavior,
SQLite representation, or reconciliation policy.

## Fixed Decisions

- `SystemName` belongs to the `System` domain. `ApplicationName` remains in the
  `Application` domain.
- Desired runtime intent belongs to `Application`; concrete runtime identity,
  endpoint, lifecycle, observation, and runtime/health specifications belong to
  `Runtime`.
- A full immutable Git commit is one validated domain value. Git resolution, OCI
  commit-tag lookup, and Deployment source revision share that value rather than
  revalidating or passing unrelated strings.
- Stores own SQL only for their aggregate. System queries live in `system_store`,
  Application intent and deployment-specification reads live in
  `application_store`, Deployment history and promotion queries live in
  `deployment_store`, and Exposure persistence lives in `exposure_store`.
- Exposure hydration, transitions, diagnostics, and reconciliation CAS writes
  move together to `exposure_store`. Application deployment-specification
  projection remains in `application_store`.
- Internal and public promotion use cases own the ordering of their writes. They
  open one short transaction, call store primitives, and treat every zero-row
  compare-and-set as an explicit stale outcome.
- Generic Application lookup is not owned by list-specific use cases. The legacy
  direct Podman candidate-creation API is either removed after migrating its
  callers or isolated as an explicitly legacy boundary.
- This refactor preserves persisted SQLite values, migration history, external-effect
  ordering, and existing reconciliation decisions. Verbose deployment commands gain
  their already-defined step-by-step stderr progress output; other CLI behavior is preserved.
- Manifest parsing retains its serde-facing DTO for input diagnostics, but provides a
  typed import projection so validated values are not reconstructed by use cases.
- Store APIs retain `SystemName`, `SystemId`, `ApplicationId`, and
  `RuntimeInstanceId` until SQLite parameters. Operation ownership tokens remain
  `String`: they are opaque generated fencing values, not domain identities.

## Checkpoint Order

1. Move System naming to `System`, desired runtime intent to `Application`, and
   runtime and health specifications to `Runtime` without behavior changes.
2. Introduce the shared immutable Git commit value across Git, OCI, and
   Deployment source revision handling.
3. Move System SQL, Application runtime intent, and Deployment-only queries to
   their owning stores; remove obsolete Release-store APIs.
4. Extract Exposure persistence, hydration, transitions, and reconciliation CAS
   primitives into `exposure_store`.
5. Move internal and public promotion write ordering into their use cases while
   retaining one atomic transaction and explicit stale outcomes.
6. Move generic Application lookup out of list-specific use cases and remove or
   isolate the legacy direct Podman candidate-creation API.
7. Extract shared remote import, diagnostics, database, and progress-enabled
   deployment orchestration from `main.rs` without changing CLI behavior.
8. Preserve validated manifest values through typed import inputs without changing
   import behavior or SQLite text.
9. Move Git repository classification and source-location values to `Git`, and
   move external container identity to `Runtime`, eliminating duplicate
   classification without changing behavior or persisted text.
10. Require typed Application, System, Deployment, and name values at the
    `application_store` boundary, converting to SQLite text only in its SQL
    parameters.
11. Require typed Application, Release, Deployment, and RuntimeInstance values
   at the Release, Deployment, and Runtime store boundaries, converting to
   SQLite text only in their SQL parameters.
12. Require typed System and Exposure store identities while retaining operation
    ownership tokens as opaque `String` values.

## Validation

- Focused tests cover moved domain values, store hydration, compare-and-set stale
  paths, promotion transaction behavior, and legacy API migration or isolation.
- CLI and integration coverage confirms no command output, exit behavior, SQLite
  representation, or external-effect ordering changes.
- Every checkpoint passes `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`.

## Non-goals

- No schema migration, automatic data repair, new deployment policy, or changed
  reconciliation behavior.
- No new CLI command, user interface, API, async runtime, dependency, or generic
  abstraction.
- No unrelated module reorganization beyond the ownership boundaries above.
