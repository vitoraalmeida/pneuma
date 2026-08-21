# Current Iteration

**Status:** completed

**Base:** `d089700` (`docs(iteration): close v0.4.1 domain hardening sweep`)

**Approved design:** [`domain-type-closure.md`](../design/domain-type-closure.md)

## Iteration - v0.4.2 Domain Type Closure

Objective: close the remaining gaps where business vocabulary crossed use-case
and adapter boundaries as primitive strings, even though validated domain types
already existed, eliminating duplicate validation, re-parsing, and unwrapping.

## Checkpoints

- [x] Type runtime lifecycle inputs with `ApplicationName`.
  Result: `report_application_status`, `stop_application`, `start_application`,
  `transition_application`, and `retire_previous_runtime` accept
  `&ApplicationName`; CLI callers pass the typed name directly.
- [x] Thread `&OciArtifact` through OCI deployment without re-parsing.
  Result: `deploy_oci*`, `pull_image`, and rollback consume `&OciArtifact`;
  parsing happens once at the CLI edge or branch-resolution boundary;
  `PullImageError::InvalidReference` is removed.
- [x] Accept typed container, health, and Quadlet identities at adapter
  boundaries.
  Result: `observe_container(&ContainerId, ContainerPort)`,
  `resolve_container_id -> ContainerId`, `observe_named_container` takes
  `ContainerPort`, `check_external_health(&DomainName, &HealthCheckPath,
  HealthCheckStatus)`, and Quadlet `unit_name`/`container_name`/
  `canonical_unit_contents`/`write_unit` take `ApplicationName`, `DeploymentId`,
  `OciArtifact`, and `ContainerPort`; duplicated validation removed.
- [x] Introduce `HostPort` for published loopback ports.
  Result: `HostPort` newtype lives in `domain/runtime.rs`;
  `reserve_port -> HostPort`; `StartedCandidate.port`,
  `ExpectedRuntimeEndpoint::host_port`, and Quadlet `host_port` parameters are
  typed; SQLite storage still uses `u16`.

## Scope and Non-goals

- No new feature, CLI command, schema migration, or persisted representation
  change.
- No new dependency, async code, trait abstraction, or generic boundary.
- No reconciliation, networking, topology, or v0.5 work.
- No new domain types for concepts that do not yet exist (`BranchName`, owner
  token, failure code/message).

## Acceptance Criteria

- Runtime lifecycle use cases accept typed `ApplicationName`.
- `OciArtifact` is parsed once at the CLI edge and branch-resolution boundary.
- Local runtime observation and external health checks accept typed identities
  and no longer duplicate validation.
- Quadlet rendering consumes typed application, deployment, artifact, and port
  identities.
- Reserved loopback host ports are represented by `HostPort` in use cases and
  adapters.
- The exact CI gates are green before closure.

## Closure Evidence

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features` (3 ignored environment-dependent Podman cases),
  and `cargo build --workspace --release` passed on the final code commit
  `d7b4277`.
- Implementation delivered in four commits:
  `d96b6ec`, `b11623a`, `d6dbeaa`, `d7b4277`.
- No VM regression prerequisite was available for this closure; the 3 ignored
  Podman environment tests were recorded as SKIP.

## Blockers

- None.
