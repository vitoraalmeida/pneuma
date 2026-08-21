# Design - Domain Type Closure

**Status:** approved design for v0.4.2. It does not describe implemented
behavior. Execution and progress live only in
[`../iterations/current-iteration.md`](../iterations/current-iteration.md).

## Objective

Close the remaining gaps where business vocabulary crossed use-case and adapter
boundaries as primitive strings, even though validated domain types already
existed. Eliminate duplicate validation, re-parsing, and unwrapping at those
boundaries.

## Fixed Decisions

- Runtime lifecycle use cases (`report_application_status`, `stop_application`,
  `start_application`, `transition_application`, and candidate cleanup) accept
  `&ApplicationName` instead of `&str`; callers already hold the typed name.
- `OciArtifact` is parsed once at the CLI edge for `--image` deploys and once at
  the branch-resolution boundary for `--branch` deploys. `deploy_oci*`,
  `pull_image`, and rollback consume `&OciArtifact`; no use case or adapter
  re-parses a raw reference.
- Container observation takes typed identities: `observe_container(&ContainerId,
  ContainerPort)`, `resolve_container_id` returns `ContainerId`, and
  `observe_named_container` accepts `ContainerPort`. The local runtime adapter no
  longer revalidates container-id format or non-zero ports.
- External health checks accept typed inputs: `check_external_health(&DomainName,
  &HealthCheckPath, HealthCheckStatus)`. The adapter does not revalidate domain
  labels, path shape, or status range.
- Quadlet unit and container naming, unit contents, and unit writing take typed
  identities: `&ApplicationName`, `&DeploymentId`, `&OciArtifact`, `ContainerPort`.
- A new `HostPort` newtype represents the loopback host port reserved by
  `port_allocator` and carried by `StartedCandidate` and
  `ExpectedRuntimeEndpoint`. SQLite storage continues to use `u16`; conversion
  happens only at the store boundary.
- This iteration preserves SQLite representation, CLI commands, persisted text,
  migration history, and external-effect ordering. There are no intentional
  behavior changes beyond removing duplicated validation.

## Checkpoint Order

1. **ApplicationName in runtime lifecycle** — type `application_runtime` and
   `deployment_runtime_cleanup` signatures with `&ApplicationName`; update CLI
   callers.
2. **OciArtifact through OCI deployment** — accept `&OciArtifact` in
   `deploy_oci*`, `pull_image`, and rollback; parse only at the CLI edge and
   branch-resolution boundary; remove `PullImageError::InvalidReference`.
3. **Typed adapter boundaries** — type `observe_container`,
  `resolve_container_id`, `observe_named_container`, `check_external_health`, and
   Quadlet APIs; remove duplicated validation.
4. **HostPort newtype** — introduce `HostPort`; make `reserve_port` return it;
   carry it through `StartedCandidate`, `ExpectedRuntimeEndpoint::host_port`, and
   Quadlet `host_port` parameters.

## Validation

- Focused tests cover typed-identity round-trips and the absence of duplicated
  validation.
- CLI and integration coverage confirms no command output, SQLite
  representation, migration behavior, or external-effect ordering changes.
- Every checkpoint passes `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, and `cargo build --workspace --release`.
- Iteration closure runs the full VM regression suite (`test-all.sh` and
  `reconciliation-e2e.sh`) on a disposable clone when the environment is
  available.

## Non-goals

- No new feature, CLI command, schema migration, or persisted representation
  change.
- No new dependency, async code, trait abstraction, or generic boundary.
- No reconciliation, networking, topology, or v0.5 work.
- No new domain types for concepts that do not yet exist (`BranchName`, owner
  token, failure code/message).
