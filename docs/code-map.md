# Pneuma Code Map

Fast navigation index: pick the flow you want to understand and read its files
in order. This is a map, not an explanation — layer responsibilities live in
[`architecture/architecture.md`](architecture/architecture.md), per-layer flow
tables and test maps in [`code-guide.md`](code-guide.md), domain vocabulary in
[`core-concepts.md`](core-concepts.md).

## Shared invocation skeleton

Every command starts the same way:

1. `src/main.rs::main` loads `/etc/pneuma/environment`
   (`load_host_environment`) and derives uid-scoped runtime variables
   (`configure_runtime_environment`).
2. `src/cli/args.rs` parses the Clap tree into the normalized `Command`.
3. `src/cli/mod.rs::run` opens SQLite (`src/adapters/database.rs`) unless the
   command needs none (`version`, `doctor`, database backup/restore, CI
   dispatch) or it routes through the interface-neutral control boundary
   (`src/control/::ControlExecutor`, which owns the database-wide lock and
   connection lifetime per command — currently the `system` family), then
   dispatches to one capability handler.
4. Handlers resolve application names through `src/cli/shared.rs::resolve_application`
   (→ `src/use_cases/application/lookup.rs::find_application_by_name`) and
   render through `src/cli/output.rs`; errors become classified `CliError`s in
   `src/cli/error.rs`.

Every mutation of an existing Application holds its per-application `flock`
(`src/adapters/application_lock.rs::ApplicationLock::try_acquire_for_connection`)
from its first state-dependent read through confirmation and compensation. The
database itself is guarded by a database-wide `flock`
(`src/adapters/database.rs::DatabaseLock`): normal commands share it for as long
as they hold their connection, restore takes it exclusively, and `version` stays
lock-free.

## Application import

Command: `pneuma app import <repository>`

Start:
`src/cli/application.rs::run_import`

Happy path:
- `src/use_cases/application/remote_import.rs::import_remote_application` —
  remote-only guard, clone into `$PNEUMA_WORKSPACE_PATH/imports/<pid>-<nanos>`
- `src/use_cases/application/import.rs::import_application` — parse manifest,
  then one transaction: existing-name short-circuit →
  `system_store::create_or_load` → `application_store::insert_application` →
  `persist_specification` (delivery/source/runtime/health/exposure specs)

Important branch: re-importing an existing name returns it unchanged
(idempotent); system name must come from flag or manifest (`SystemRequired`).

Domain rules: `src/domain/manifest.rs::ImportSpecification` (validated input);
catalog value objects (`ApplicationName`, `RelativeManifestPath`).

Adapters: `src/adapters/git_source.rs::clone_repository`,
`src/adapters/manifest.rs::load_manifest_at`.

Failure/recovery: clone failure cleans the checkout
(`cleanup_checkout`, always attempted); store errors abort the transaction so
no partial specification becomes visible.

## Application start

Command: `pneuma app start <app>`

Start: `src/cli/application.rs::run_start`

Happy path: `src/use_cases/application/runtime.rs::start_application` → shared
controller `transition_application`: persist desired state first
(`set_desired_state`, CAS) → observe → supervised unit start.

Important branch: container missing → `handle_missing_runtime` recreates it
under the stable Quadlet name instead of failing.

Domain rules: `src/domain/application.rs::DesiredRuntimeState`;
`src/domain/runtime.rs::ObservedRuntimeState`.

Adapters: `src/adapters/systemd_quadlet.rs::{unit_exists, start}`;
`src/adapters/local_runtime.rs::{observe_container, start_container}` as the
direct fallback without supervision.

Failure/recovery: `RuntimeLifecycleError`; intent is already persisted, so
status and reconciliation still observe it after an interrupted control.

## Application stop

Same controller with `RuntimeCommand::Stop`
(`src/use_cases/application/runtime.rs::stop_application`).

Important branch: Quadlet removes the container on ExecStop;
`missing_container_satisfies_stop_intent` records that as a completed stop.
Everything else matches **Application start**.

## Deploy from branch

Command: `pneuma app deploy <app> --branch <b>`

Start: `src/cli/deployment.rs::run_deploy_branch`

Resolution: `src/use_cases/deployment/deploy.rs::deploy_branch`
- `application_store::load_source` (manifest default branch as fallback)
- `git_source::resolve_branch` to an immutable commit
- `application_store::load_delivery_specification` →
  `oci_image::resolve_image_digest`
- hands the loaded policy to `deploy_artifact_for_delivery` with the commit
  recorded as provenance

Then the execution spine under **Deploy from OCI**.

Domain rules: `src/domain/git.rs::CommitSha` recorded directly as the
deployment's optional source revision (`src/domain/deployment.rs`).

Failure path: `DeployBranchError` distinguishes missing source configuration,
no default branch, and no delivery configuration before any external effect.

## Deploy from OCI

Command: `pneuma app deploy <app> --image <ref>`

Start: `src/cli/deployment.rs::run_deploy_oci` — parses
`OciArtifact::parse` before any effect; assembles
`PublicDeploymentConfiguration` from the Caddy paths.

Resolution: `src/use_cases/deployment/deploy.rs::deploy_oci` →
`deploy_artifact_for_delivery`:
`DeliverySpecification::permits` allow-list → `pull_image` →
`src/use_cases/release/mod.rs::create_release_while_locked` (digest-pinned reuse).

Execution spine (`src/use_cases/deployment/execute.rs::deploy_release_reporting`):

```text
application lock
→ pending deployment row (create.rs::create_deployment_with_source_revision_while_locked)
→ execute_deployment → candidate.rs::start_candidate:
    advance_deployment(Start) → port_allocator::reserve_port
    → systemd_quadlet::{write_unit, daemon_reload, start}
    → local_runtime::{resolve_container_id, observe_container}
    → register_candidate_runtime
→ visibility branch: internal or public finish
→ cleanup.rs::retire_previous_runtime
```

Domain rules: `src/domain/release.rs` (`OciArtifact` grammar, permits);
`DeploymentStatus::transition` table (`src/domain/deployment.rs`);
`Visibility` selects the finish variant.

Adapters: `src/adapters/oci_image.rs`, `port_allocator.rs`,
`systemd_quadlet.rs`, `local_runtime.rs`.

Failure path: every step tags its allocated resources in `CandidateResources`
and routes through the finalizer described under **Failed deployment /
compensation**.

## Internal deployment

Finish variant of the deploy spine for `Visibility::Internal`:
`src/use_cases/deployment/execute.rs::finish_internal_deployment` →
`src/use_cases/deployment/promotion/internal.rs::promote_internal_candidate`:

1. pre-validate the target (`ensure_internal_promotable`)
2. loopback health via `check_internal_health`; unhealthy ⇒ `fail_deployment`
3. one immediate transaction: `stop_other_running_runtimes` → `start_runtime`
   → `mark_succeeded` → `activate_deployment`, each CAS-guarded

Domain rule: exactly one active runtime per application, enforced by guarded
writes rather than trust.

## Public deployment

Finish variant for `Visibility::Public`:
`src/use_cases/deployment/execute.rs::finish_public_deployment` →
`src/use_cases/deployment/activation.rs::activate_public_candidate`, in order:

```text
verify_internal_health → mark_activating
→ materialize_public_route (caddy_exposure::materialize_caddy_fragment)
→ verify_external_health_or_rollback (health_check_external)
→ promote_public_runtime_or_rollback
   (promotion/public.rs::begin_public_exposure + promote_public_candidate)
```

Compensation: any later-step failure restores the previous Caddy state
(`rollback_public_route`) before resources join the common finalizer.

Domain rules: route evidence and configuration versions
(`src/domain/exposure.rs`); health expectations
(`src/domain/runtime.rs::HealthCheckSpecification`).

Adapters: `src/adapters/caddy_exposure.rs`,
`src/adapters/health_check_internal.rs`, `src/adapters/health_check_external.rs`.

## Failed deployment / compensation

Finalizer: `src/use_cases/deployment/failure.rs::finish_failed_deployment`

1. `persist_failure_if_needed` — record the failure stage through
   `transition.rs::fail_deployment` unless promotion already persisted it
2. `cleanup_candidate_if_needed` — release unit, container, and reserved port
   (`cleanup.rs::cleanup_failed_candidate`)
3. `resolve_failure_recovery` — report by precedence: cleanup divergence, then
   failure-recording divergence, then the original failure

Domain rules: typed failure codes (`failure.rs::DeploymentFailureCode`),
lifecycle transition legality (`DeploymentEvent`).

Recovery note: interrupted deployments are finished by reconciliation instead —
see below.

## Rollback

Command: `pneuma deployment rollback <app>`

Start: `src/cli/deployment.rs::run_rollback`

Happy path: `src/use_cases/deployment/rollback.rs::rollback_deployment`
- existence check → target selection `previous_release`
  (`deployment_store::load_rollback_target`)
- re-pull the historical artifact (`oci_image::pull_image`)
- run `execute.rs::deploy_release` as `DeploymentType::Rollback` — the normal
  spine, preserving history

Important branch: no previous succeeded deployment ⇒ `NoPreviousDeployment`.

Domain rules: newest succeeded non-active deployment, provenance preserved
(`RollbackTarget` in `src/domain/deployment.rs`).

## Visibility change

Command: `pneuma app visibility set <app> public|internal`

Start: `src/cli/exposure.rs::run_visibility_set`

Happy path: `src/use_cases/exposure/mod.rs::change_exposure`
1. same-visibility short-circuit
2. `begin_change` — CAS intent reservation before any Caddy effect
3. `make_public`: require domain, active successful runtime observed Running →
   materialize fragment → external health → confirm in one immediate
   transaction (`complete_public_exposure_change`)
4. `make_internal`: `remove_caddy_fragment`, leaving the loopback runtime alone

Important branch: concurrent exposure change wins ⇒ CAS returns stale ⇒
conflict error, never overwrite.

Domain rules: `ExposureIntent` (public requires a `DomainName`),
`ExposureMaterializationState` machine (`src/domain/exposure.rs`).

Adapters: `src/adapters/caddy_exposure.rs` (materialize/remove/observe plus
`restore_*` compensations), `src/adapters/local_runtime.rs` observation,
`src/adapters/health_check_external.rs`.

Failure/recovery: compensate Caddy first, then record a diagnostic with
`record_failure` (CAS-guarded) before returning.

## Reconciliation

Command: `pneuma reconcile <app>`

Start: `src/cli/reconciliation.rs::run_reconcile`

Pipeline (`src/use_cases/reconciliation/mod.rs::reconcile_application`):
1. application lock
2. blocking deployment present? → `recover.rs::recover_interrupted_deployment`
3. otherwise: `load.rs::load_reconciliation_input` (persisted facts) →
   `observe.rs::observe_reconciliation_input` (Podman, Quadlet, Caddy) →
   pure decision `src/domain/reconciliation/decision.rs::decide` →
   `execute.rs::execute_reconciliation_decision`

Execution variants: runtime effects in `runtime_effects.rs` (identity repair
`swap_recorded_container_id`, rematerialization `rematerialize_runtime`) and
exposure effects in `exposure_effects.rs` (route removal/materialization with
restore-on-failure compensation).

Outcome vocabulary: `ReconciliationResult` — `NoOp`, `Deferred`, `Repaired`,
`ExposureRepaired`, `ManualIntervention`, `Failed`, `Diverged`.

Domain rules: the pure decision matrix in `src/domain/reconciliation/decision.rs`
with conservative precedence; ambiguous materializations stay untouched.

## Status and inspection

Read-only flows; none of them mutate operator intent:

- status: `src/cli/application.rs::run_status` →
  `runtime.rs::report_application_status` — observe Podman, persist the
  observation only
- application list: `run_list` → `list_applications` +
  `application_is_deployed`
- deployment history: `src/cli/deployment.rs::run_deployments` →
  `deployment/query.rs::list_deployments`
- systems: `src/cli/system.rs` → `src/control/::ControlExecutor` →
  `use_cases/system/{create,list,show}.rs`
- host diagnostics: `src/cli/doctor.rs::run_doctor`; version needs no database
  (`src/cli/mod.rs::run_version`)

Adapters: `local_runtime::observe_container` for live state; stores otherwise.

## SSH CI dispatch

Still a supported major flow: the restricted SSH forced command
(`pneuma ci dispatch`, documented in getting-started).

Start: `src/cli/ci.rs::run_ci_dispatch`

Happy path: read `SSH_ORIGINAL_COMMAND` → validate with
`src/use_cases/ci/mod.rs::parse_ci_command` (only `version` or
`deploy <application> <branch>`; shell metacharacters rejected) → dispatch to
`run_deploy_branch`.

Domain rules: reuse of the catalog `ApplicationName` rule at the SSH boundary.
