# Pneuma Code Guide

**Status:** living document - describes the code layout as implemented in v0.4.3.

This guide is for a new contributor who needs to follow one user-facing flow
end to end without global searching. It maps each flow through the layers:

```text
CLI entry → use case → domain rules → stores → external adapters → tests
```

Deeper reference material lives in [`architecture/architecture.md`](architecture/architecture.md)
(layer responsibilities), [`architecture/invariants.md`](architecture/invariants.md)
(the authoritative `INV-*` invariant inventory cited below), and
[`architecture/data-model.md`](architecture/data-model.md) (persisted schema).

## Repository Layout

```text
src/main.rs                  process bootstrap and composition root only
src/config.rs                documented PNEUMA_* path variables, path resolution, verbose logging
src/cli/                     argument tree, dispatch, handlers, output, error classes
src/use_cases/<capability>/  workflow ordering and external-effect orchestration
src/domain/                  value objects, entities, transitions, pure policy
src/adapters/stores/         SQLite encoding, CAS primitives, transactions
src/adapters/*.rs            Git, OCI, Podman, systemd Quadlet, Caddy, health, ports, flock
tests/                       integration and binary-level CLI tests (tests/cli.rs)
```

One crate: a library (`src/lib.rs`) plus a thin binary entrypoint. There is no
async; every external command integration is a child process except SQLite,
filesystem work, and internal TCP health checks.

## Shared Invocation Path

Every normal command follows the same skeleton before reaching its flow:

1. `src/main.rs` loads host configuration from `/etc/pneuma/environment`
   (never overriding caller-supplied variables), derives the uid-scoped runtime
   environment (`XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, default
   `PNEUMA_QUADLET_DIR`), then calls `cli::run(cli::parse_invocation())`.
2. `src/cli/args.rs` owns the Clap tree and translates it into the normalized
   `Command` enum.
3. `src/cli/mod.rs::run` opens the SQLite connection (except version, doctor,
   backup/restore, and CI dispatch) and dispatches to one capability handler.
4. Handlers in `src/cli/{system,application,deployment,exposure,reconciliation}.rs`
   validate CLI input, resolve names through `shared::resolve_application`,
   call exactly one use-case entry point, and render with `output.rs`.
5. Failures map to `CliError` variants in `error.rs`; `CliError::class()`
   assigns stable exit codes (1 failure, 2 usage, 3 not-found, 4 conflict,
   5 external).

One coordination mechanism applies to every existing-Application mutation:
`adapters/application_lock.rs::ApplicationLock::try_acquire_for_connection` is a
kernel `flock` per Application, held from the first state-dependent read through
effects, confirmation, and compensation. A failed acquire is an explicit
conflict; reconciliation returns `Deferred`.

---

## Flow 1: System — `pneuma system create|list|show`

| Layer | Where |
|---|---|
| CLI entry | `cli/system.rs` (`run_system_create`, `run_system_list`, `run_system_show`) |
| Use cases | `use_cases/system/create.rs`, `list.rs`, `show.rs` |
| Domain rules | `domain/system.rs`: `SystemName` catalog-name validation; `SystemDetails` read model built by `show.rs` |
| Stores | `stores/system_store.rs` (`create_or_load` is idempotent by name: same id, original description preserved) |
| External adapters | none — pure persistence flow |
| Tests | `tests/system_show.rs`; store round-trip and corruption tests inside `system_store.rs`; CLI scenarios in `tests/cli.rs` |

## Flow 2: Import — `pneuma import <repository>`

| Layer | Where |
|---|---|
| CLI entry | `cli/application.rs::run_import` (resolves `PNEUMA_WORKSPACE_PATH`) |
| Use cases | `application/remote_import.rs::import_remote_application` → `application/import.rs::import_application` |
| Domain rules | `domain/manifest.rs::ImportSpecification` (validated use-case input); value objects built at the boundary (`ApplicationName`, `RelativeManifestPath`, delivery/runtime specifications); re-import returns the existing application unchanged |
| Boundary adapter | `adapters/manifest.rs` — private TOML structs, `load_manifest_at` = parse + validate + convert in one step (INV-MAN-001); `adapters/git_source.rs::clone_repository` / `cleanup_checkout` (always attempted) |
| Stores | `application_store` (`generate_id`, `insert_application` — writes the required `system_id` once), `system_store::create_or_load`, specification writers invoked by `persist_specification` |
| External adapters | `git_source` (clone into `workspace/imports/<pid>-<nanos>`) |
| Tests | `tests/manifest.rs` (parsing/validation), `tests/application_import.rs` (idempotency, mid-aggregate rollback), `tests/application_specification.rs`, `tests/git_source.rs`, E2E import scenarios in `tests/cli.rs` |

## Flow 3: Deploy OCI — `pneuma app deploy <app> --image <ref>`

The richest workflow; read top-down through `use_cases/deployment/` whose
`mod.rs` documents each submodule.

| Layer | Where |
|---|---|
| CLI entry | `cli/deployment.rs::run_deploy` selects OCI vs branch; `run_deploy_oci` parses `OciArtifact::parse` **before** any effect and assembles `PublicDeploymentConfiguration` (Caddy paths) |
| Use cases | `deployment/deploy.rs::deploy_oci` (Application lock → delivery check → pull → release) → `release/create_release_while_locked` (digest reuse) → `deployment/execute.rs::deploy_release_reporting` (pending deployment → candidate execution → promotion or finalized failure) → `deployment/candidate.rs::start_candidate` (port → unit → start → observe → register) → `promotion/internal.rs::promote_internal_candidate` (internal) or `activation.rs::activate_public_candidate` (public) |
| Domain rules | `domain/release.rs`: `OciArtifact` digest grammar, `DeliverySpecification::permits` repository allow-list (INV-REL-002); `domain/deployment.rs`: single transition table `DeploymentStatus::transition(DeploymentEvent)` (INV-DEP-001); `Visibility` decides internal-vs-public activation path |
| Stores | `application_store` (deployment specification load), `release_store` (digest-pinned reuse), `deployment_store` (pending insert under partial unique index INV-DB-001, targeted CAS status advance, guarded `activate_deployment` INV-APP-002), `runtime_store` (candidate registration, tombstones), port reservations via `port_allocator` |
| External adapters | `oci_image::pull_image` (digest verify), `port_allocator::reserve_port`, `systemd_quadlet` (`write_unit`, `daemon_reload`, `start`), `local_runtime` (`resolve_container_id`, `observe_container`), `health_check_internal` (loopback), `caddy_exposure::materialize_caddy_fragment` + external health (public only) |
| Failure path | every allocated resource is tracked in `CandidateResources`; one finalizer (`finish_failed_deployment`) records the typed failure via `fail_deployment` then removes container/unit/port in order |
| Tests | `tests/deployment_from_oci.rs` (repository mismatch before pull, success/failure), `tests/release_create.rs`, `tests/deployment_execute_release.rs` (workflow ordering, compensation), adapter contract tests inside `oci_image.rs` / `local_runtime.rs` / `systemd_quadlet.rs`, E2E deploy scenarios in `tests/cli.rs` (fake binaries also prove no transaction is open during effects) |

## Flow 4: Deploy Revision — `pneuma app deploy <app> --branch <b>`

Identical to Flow 3 after source resolution; only the front differs.

| Layer | Where |
|---|---|
| CLI entry | `cli/deployment.rs::run_deploy_branch` |
| Use cases | `deployment/deploy.rs::deploy_branch`: load persisted source configuration (`application_store::load_source`, falling back to the manifest default branch) → resolve commit → load delivery configuration → resolve digest → hand the loaded delivery policy to `deploy_artifact_for_delivery` with `source_commit` (the shared validated path behind `deploy_oci`, so the specification is never re-read) |
| Domain rules | `domain/git.rs::CommitSha` validated hex; resolved commit is recorded directly as an optional `CommitSha` on the deployment (`domain/deployment.rs`) |
| Stores | `application_store::load_source` / `load_delivery_specification`; everything downstream identical to Flow 3 |
| External adapters | `git_source::resolve_branch` (ls-remote against the repository URL), `oci_image::resolve_image_digest` (`repo:<sha>` pull + inspect) |
| Tests | `tests/deployment_from_revision.rs`, `tests/git_source.rs` (transport classification, resolution errors) |

## Flow 5: Rollback — `pneuma deployment rollback <app>`

| Layer | Where |
|---|---|
| CLI entry | `cli/deployment.rs::run_rollback` |
| Use cases | `deployment/rollback.rs::rollback_deployment`: application existence check → select target → re-pull historical artifact → run the normal release deployment as type `Rollback` |
| Domain rules | target selection rule "newest succeeded deployment that is not currently active" implemented by `deployment_store::load_rollback_target` and returned as a domain `RollbackTarget` carrying provenance (an optional `CommitSha`) |
| Stores | `deployment_store` (history query), then the full Flow 3 store set with `DeploymentType::Rollback` |
| External adapters | `oci_image::pull_image` for the historical artifact, then all Flow 3 runtime/exposure adapters |
| Tests | `tests/deployment_rollback.rs` (target selection), rollback happy-path E2E in `tests/deployment_execute_release.rs` (new rollback deployment, activation, retirement of the replaced runtime) |

## Flow 6: Exposure — `pneuma app visibility set <app> public|internal`

| Layer | Where |
|---|---|
| CLI entry | `cli/exposure.rs::run_visibility_set` (assembles Caddy paths) |
| Use cases | `exposure/mod.rs::change_exposure`: same-visibility short-circuit → CAS intent reservation **before** any Caddy effect → `make_public` / `make_internal` with restore compensation on later failure |
| Domain rules | `domain/exposure.rs`: `ExposureIntent::new` (public requires a `DomainName`), `ExposureMaterializationState` state machine (Applying/Active/Removing), `ConfirmedRoute` evidence |
| Stores | `exposure_store`: `begin_exposure_change` CAS on expected visibility (Stale ⇒ conflict, never overwrite), completion primitives conditioned on the reservation state (INV-DB-004) |
| External adapters | `caddy_exposure`: canonical fragment bytes, `materialize_caddy_fragment`, `observe_caddy_fragment`, `remove_caddy_fragment`, `restore_*` compensations, reload after each change |
| Tests | `tests/caddy_exposure.rs` (13 fake-caddy scenarios incl. removal-of-absent and restore), exposure store CAS tests inside `exposure_store.rs`, visibility E2E scenarios in `tests/cli.rs` |

## Flow 7: Start/Stop/Status — `pneuma app start|stop|status <app>`

| Layer | Where |
|---|---|
| CLI entry | `cli/application.rs::run_start` / `run_stop` / `run_status` |
| Use cases | `application/runtime.rs`: `report_application_status` (observe + persist observation, never changes intent), `stop_application` / `start_application` both funnel into the shared `transition_application` controller |
| Domain rules | `domain/application.rs::DesiredRuntimeState`; `domain/runtime.rs`: `ObservedRuntimeState` (unknown Podman states preserved as `Unknown`), `ContainerId`, loopback-only `ExpectedRuntimeEndpoint`; intent is persisted before any external effect |
| Stores | `application_store::set_desired_runtime_state` (under the Application lock), `runtime_store` (active-runtime load, observation writes, hydratable tombstones `state='removed'`) |
| External adapters | `local_runtime::start_container` / `stop_container` / `observe_container`; `systemd_quadlet::unit_name` on the missing-container recreation path (Quadlet recreates containers under a stable name) |
| Tests | lifecycle E2E scenarios in `tests/cli.rs` (idempotent stop/start, stop after container removal, start after removal recreating via Quadlet), runtime hydration/tombstone tests inside `runtime_store.rs` and `tests/deployment_register_runtime.rs`, VO boundaries in `tests/domain_values.rs` |

## Flow 8: Reconcile — `pneuma reconcile <app>`

Pipeline shape (see `use_cases/reconciliation/mod.rs` module docs):

```text
application lock → recover branch → load → observe → decide (pure) → execute
```

| Layer | Where |
|---|---|
| CLI entry | `cli/reconciliation.rs::run_reconcile` |
| Use cases | `reconciliation/mod.rs::reconcile_application` (ordering + compensation only); `recover.rs` (interrupted-deployment recovery), `load.rs` (persisted facts), `observe.rs` (external facts + boundary-rendered canonical expectations), `execute.rs` (decision translation, identity repair, rematerialization confirmation, exposure reserve/materialize/remove/failure recording) |
| Domain rules | `domain/reconciliation.rs::decide` — pure function, no infrastructure imports; answers "what should happen?" over desired/persisted/observed facts with conservative precedence (stopped-in-sync → runtime identity repair → rematerialization → exposure classification → manual intervention) |
| Stores | read-side loads across application/deployment/runtime/exposure stores; writes are CAS-guarded (runtime identity repair, exposure reservations/completions) |
| External adapters | `local_runtime` (container observation), `systemd_quadlet` (`observe_generated_unit`, unit rewrite), `caddy_exposure` (fragment observation/materialization/removal), `health_check_internal` after rematerialization |
| Tests | decision matrix (25 cells) in-file at `src/domain/reconciliation.rs`; `tests/reconciliation.rs` (ownership deferral, blocking deployments, interrupted candidates, proven-route preservation); lost-completion-CAS restore scenarios in `tests/cli.rs` |

---

## Conventions To Keep In Mind While Reading

- Public surface is minimal: capability `mod.rs` files curate what is exported;
  internal steps are private or `pub(crate)`. If an item is `pub`, an external
  consumer exists.
- Error boundaries: technical errors originate in the adapter or operation that
  produced them and keep that vocabulary. Deployment failure classification
  happens once, at the deployment use-case boundary: `FailedExecution` (internal
  to `use_cases/deployment`) combines the semantic `DeploymentFailureCode` with
  the source error, compensation resources, and persistence state. Failure
  finalization is centralized in `finish_failed_deployment`, whose recovery
  precedence is cleanup divergence, then failure-recording divergence, then the
  original failure. The CLI owns presentation and exit classification and never
  recovers semantics by parsing error text.
- Zero-row CAS updates mean stale/conflict (`PersistenceOutcome::Stale`),
  never success (INV-DB-004).
- No SQLite transaction is held across Git/OCI/Podman/systemd/Caddy/HTTP work;
  intent commits first, confirmation transactions open after observation
  (INV-WF-002).
- Rule citations: when behavior references an invariant, look up its `INV-*-nnn`
  identifier in [`architecture/invariants.md`](architecture/invariants.md)
  instead of re-deriving the rule from prose.
- Test levels follow [`docs/rust-guidelines.md`](rust-guidelines.md): domain
  matrices live in-file next to the domain code, store/adapter contracts live
  in-file with their modules, cross-module ordering and E2E behavior lives in
  `tests/*.rs` driven through real binaries with faked external commands.
