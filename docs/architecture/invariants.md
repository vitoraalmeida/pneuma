# Pneuma Invariant Inventory

**Status:** living document - the authoritative inventory of architectural
invariants, produced by consolidation iteration 01.

Every relevant architectural rule is listed here with its category, its current
owner in code, its desired owner, its secondary defense line, and the tests that
prove it. No rule of the architecture may remain only implicit; if a rule is
missing from this table, either add it or record why it does not apply.

## How to Read This Table

Categories follow the consolidation classification:

- **Value invariant** - determined from a single value; owned by a validated
  domain type constructed once at the boundary.
- **Entity invariant** - determined by an entity plus the requested operation;
  owned by entity behavior or a pure domain function.
- **Cross-object rule** - depends on more than one domain object; owned by a
  pure domain function or enforced with persistence support.
- **Persistence invariant** - protects against races or corruption; owned by
  SQLite constraints plus adapter CAS mechanics, with use-case cooperation.
- **Workflow invariant** - ordering of persistence and external effects; owned
  by use cases.
- **External-boundary invariant** - data or effects coming from outside; owned
  by adapters converting into safe domain types.

"Desired owner" differs from "current owner" only where the inventory found an
ownership gap; identical entries mean the rule already lives where it belongs.

| ID | Rule | Categoria | Owner atual | Owner desejado | Defesa secundária | Teste atual | Teste desejado |
|---|---|---|---|---|---|---|---|
| INV-SYS-001 | System names satisfy the shared catalog name rule: 1–63 chars, lowercase ASCII letters/digits/hyphens, alphanumeric first and last. | Value invariant | `SystemName::new` - `src/domain/system.rs:18`; shared predicate `is_valid_catalog_name` - `src/domain/identity.rs:144` | Same (domain) | Manifest import validation (`src/adapters/manifest.rs`); CLI uses `SystemName::new` (`src/cli/system.rs:17,41`) | `tests/manifest.rs::accepts_name_domain_and_status_boundaries`, `rejects_overlong_names_and_domains`; `src/use_cases/ci/mod.rs` name tables | Direct unit tests for `SystemName` boundary values in `src/domain/system.rs` |
| INV-APP-001 | Application names satisfy the same catalog name rule as Systems. | Value invariant | `ApplicationName::new` - `src/domain/application.rs:35`, predicate `src/domain/identity.rs:144` | Same (domain) | Manifest validation (`src/adapters/manifest.rs`); CI command parsing reuses it (`src/use_cases/ci/mod.rs`) | `src/use_cases/ci/mod.rs::parse_deploy_reuses_the_domain_application_rule`, `valid/invalid_application_names`; `tests/manifest.rs` | Direct unit tests for `ApplicationName` in `src/domain/application.rs` |
| INV-APP-002 | Application persisted state has exactly three mutation paths: immutable identity fields (`system_id`, `manifest_schema_version`) are written once at import and never updated; runtime intent changes only through CAS (`compare_and_set_desired_runtime_state`) or through activation; activation writes `active_deployment_id` paired with running intent only for a succeeded Deployment belonging to that Application. No code mutates a hydrated `Application`; `request_start`/`request_stop`/`activate` entity methods are deliberately absent because intent writes are ID-keyed store operations, not field mutations of a loaded entity (same rationale as INV-DEP-006). | Cross-object rule | Import insert (`insert_application`), CAS intent primitive (`compare_and_set_desired_runtime_state`), guarded activation primitive (`activate_deployment`) - all in `src/adapters/stores/application_store.rs`; eligibility decided by domain (`DeploymentStatus::transition(Activated)` gates both promote use cases) | Same split (domain decides eligibility, store enforces the guarded write) | FK `applications.active_deployment_id REFERENCES deployments(id)` - `migrations/0007_deployment_release.sql:136`; promotion transactions atomic (INV-WF-003); CAS semantics make zero-row outcomes conflicts, never silent success (INV-DB-004) | `tests/application_specification.rs::activates_a_succeeded_deployment_of_the_application_with_running_intent`, `rejects_foreign_or_unsucceeded_activation_without_changing_application_state`; promote flows via `tests/deployment_promote_internal.rs` | Keep |
| INV-APP-003 | `Application.system_id: Option<SystemId>` means exactly one thing: `None` is a legacy tolerance for rows persisted before Systems existed (migration 0005 added the nullable column via `ALTER TABLE`); it is not a valid state for new Applications. Every import writes exactly one System (`insert_application` takes `&SystemId`, so no code path can insert NULL), and hydration maps the legacy NULL to `None` so historical rows stay listed rather than failing. This is the third documented legacy tolerance of INV-DB-006, alongside `SourceRevision::Legacy` and `DeploymentFailureEvidence::Incomplete`. | Persistence invariant | Import insert requires a resolved System (`insert_application` - `src/adapters/stores/application_store.rs:79`); nullable-column hydration (`map_application_row` - `src/adapters/stores/application_store.rs:139`) | Same split (store owns encoding; domain documents the tolerance) | Column is nullable by migration immutability (INV-DB-007); FK `applications.system_id REFERENCES systems(id)` - `migrations/0005_systems.sql:8`; documented semantics in `docs/architecture/data-model.md:28-30` | `tests/application_list.rs::lists_legacy_applications_without_a_system` (legacy NULL stays listed); `tests/application_import.rs::...system_id.is_some()` plus systems join assertion (new imports always carry one System) | Keep |
| INV-APP-004 | `applications.spec_version` (domain field `Application::manifest_schema_version`) is an immutable copy of the manifest `schema_version` recorded once at import (`import_application` passes `ImportSpecification.schema_version` to `insert_application`). It is not a per-application counter, is not monotonic, is never reset or compared, and does not participate in optimistic concurrency. Imports accept only schema version 3 today (INV-MAN-001); legacy rows may persist older values (column default `1`, `CHECK (spec_version > 0)`) and hydration must tolerate them, which is why the field stays a plain `u32`: restricting construction to the currently supported version would reject legacy rows, and the nonzero rule is already enforced by SQLite. | Persistence invariant | Import insert (`insert_application`) and row hydration (`map_application_row`) - `src/adapters/stores/application_store.rs`; semantics documented on `src/domain/application.rs` fields | Same split (store owns encoding; domain documents the tolerance) | Column CHECK and DEFAULT - `migrations/0001_application_catalog.sql:6-7`; import-time gate INV-MAN-001 (`src/adapters/manifest.rs`) | `tests/application_import.rs::manifest_schema_version == 3` on fresh imports; `tests/application_list.rs::lists_legacy_applications...` (legacy row with `spec_version = 1` stays listed); `tests/system_show.rs::returns_application_runtime_intent_and_manifest_schema_version` | Keep |
| INV-MAN-001 | Manifests must declare schema version 3 exactly; unknown TOML fields are rejected. | External-boundary invariant | Schema gate in `import_specification` plus private serde deny-unknown-fields document DTO - `src/adapters/manifest.rs` | Adapter boundary (`src/adapters/manifest.rs`); domain consumes the validated `ImportSpecification` | Import use case refuses to proceed on parse error | `tests/manifest.rs::rejects_unknown_fields_and_schema_versions` (and related) | Keep; add negative test for future schema version 4 rejection |
| INV-MAN-002 | Delivery is OCI-only; `[delivery].image` is a repository without tags/digests or surrounding whitespace. | External-boundary invariant | `OciRepository::new` via `import_specification` - `src/adapters/manifest.rs`; repository grammar - `src/domain/release.rs:144-182` | Same (domain) | Delivery spec persisted with `CHECK (delivery_type IN ('oci'))` - `migrations/0011_application_delivery_specs.sql:3` | `tests/manifest.rs`; `tests/release_create.rs::rejects_invalid_artifact_identity` | Keep |
| INV-MAN-003 | Public default visibility requires a valid domain; internal may omit it. | Cross-object rule | `ExposureIntent::new` - `src/domain/exposure.rs:41-52` | Same (domain) | SQLite `CHECK (desired_visibility = 'internal' OR domain IS NOT NULL)` - `migrations/0001_application_catalog.sql:60` | `tests/manifest.rs::requires_a_domain_for_public_exposure`, `allows_internal_exposure_without_a_domain` | Keep |
| INV-REL-001 | A Release artifact identity is always `repository@sha256:<64 lowercase hex>`; mutable tags never become artifacts. | Value invariant | `OciArtifact::parse/new` - `src/domain/release.rs:24-44` | Same (domain) | Store re-validates persisted reference and cross-checks stored columns (`release_store.rs:177-189`); pull-time digest verification (`src/adapters/oci_image.rs:142-154`) | `src/adapters/oci_image.rs` parse tests; `tests/deployment_from_oci.rs::reject_unpinned_reference_at_validation_boundary`; `tests/release_create.rs::rejects_invalid_artifact_identity`; ignored registry tests in `tests/oci_image.rs` (SKIP: no rootless Podman host) | Keep; run `tests/oci_image.rs` on a configured rootless Podman host |
| INV-REL-002 | An Application permits only the OCI repository recorded from its manifest; foreign repositories are rejected before any pull. | Cross-object rule | `DeliverySpecification::permits(&OciArtifact)` - `src/domain/release.rs`; use case applies it before any effect - `src/use_cases/deployment/deploy.rs` | Same split (domain decides permission, use case orders effects) | DB ownership trigger prevents mismatched Release rows after creation (`migrations/0009_deployment_release_application.sql`) | In-file `src/domain/release.rs::permits_the_exact_configured_repository`, `rejects_foreign_and_prefix_repositories`; `tests/deployment_from_oci.rs::repository_not_allowed_before_pull` | Keep |
| INV-REL-003 | `(application_id, image_digest)` uniquely identifies a Release; duplicates are reused, never re-created. | Persistence invariant | Unique index - `migrations/0006_releases.sql`; reuse logic - `src/use_cases/release/mod.rs` | Same | Domain `OciArtifact` equality | `tests/release_create.rs::creates_and_reuses_a_release_from_one_validated_artifact`; `tests/deployment_create.rs::reuses_a_release_for_a_later_deployment_attempt` | Keep |
| INV-DEP-001 | The only legal Deployment transitions are Pending→Starting→Verifying→Activating→Succeeded plus Verifying→Succeeded (internal promotion), with every non-terminal state allowed to fail into Failed; Succeeded/Failed are terminal. | Entity invariant | `DeploymentStatus::transition/is_terminal/can_fail` over `DeploymentEvent` - `src/domain/deployment.rs:181-249` | Same (domain is the single authority) | Store CAS primitives persist only the loaded→domain-approved pair (`advance_status`, `mark_succeeded`, `mark_failed` - `src/adapters/stores/deployment_store.rs`); CHECK constraint enumerates statuses (`migrations/0007_deployment_release.sql:21`) | Full state×event matrix in `src/domain/deployment.rs` tests; `tests/deployment_transition.rs` (all 6 tests incl. `rejects_skipped_and_repeated_transitions_without_changing_state`, `terminal_and_missing_deployments_cannot_enter_the_flow`) | Keep |
| INV-DEP-002 | A newly recorded failure requires trimmed non-empty code, message, timestamp, and a non-terminal stage. | Entity invariant | `DeploymentFailure::validate_details/new` - `src/domain/deployment.rs:82-114` | Same (domain) | Hydration matrix rejects terminal evidence on non-terminal rows (`deployment_store.rs:465-520`) | `tests/deployment_transition.rs::rejects_incomplete_failure_details_without_changing_state`; `src/adapters/stores/deployment_store.rs::rejects_terminal_and_nonterminal_evidence_mismatches` | Keep |
| INV-DEP-003 | Historical failed rows with incomplete evidence hydrate as `DeploymentFailureEvidence::Incomplete` instead of being rejected or invented complete. | Persistence invariant | `deployment_store.rs` hydration - `465-520,572-586` | Adapter (legacy tolerance at the boundary) | None needed beyond typed representation | `src/adapters/stores/deployment_store.rs::preserves_incomplete_historical_failed_evidence` | Keep |
| INV-DEP-004 | Promotion requires logical state Starting, observed Podman Running, no retirement; an already-Succeeded deployment returns the confirmed promotion idempotently. | Entity invariant | `PromotionTarget::validate_promotion_candidate/completed_promotion` - `src/domain/deployment.rs:246-278` | Same (domain decides; use case executes) | Store promotion transaction is atomic and guarded by CAS | `tests/deployment_promote_internal.rs::promotes_healthy_candidate_idempotently`, `replaces_the_previous_current_runtime_atomically` | Keep |
| INV-DEP-005 | Rollback creates a new `rollback` Deployment for the most recent succeeded non-active Release and never edits prior history. | Entity invariant | `RollbackTarget` selection - `src/domain/deployment.rs:280-285`; orchestration - `src/use_cases/deployment/rollback.rs` | Same split (domain selects, use case orchestrates) | Insert-only deployment history; unique non-terminal index blocks concurrent attempts | `src/use_cases/deployment/rollback.rs::selects_provenance_from_the_historical_deployment`; guard tests in `tests/deployment_rollback.rs` | Happy-path E2E proving rollback executes a new successful Deployment (currently missing) |
| INV-DEP-006 | Every semantically relevant Deployment mutation routes through a domain operation: event transitions through `DeploymentStatus::transition`, failures through `can_fail()` + `DeploymentFailure::validate_details`, activation confirmation through the `transition(Activated)` gate. The remaining writes are deliberately procedural persistence bookkeeping: insert-only creation of Pending rows, terminal timestamp/evidence writes inside the CAS primitives, and first-start timestamping (`started_at` stamped exactly once when leaving Pending inside `advance_status`). No entity methods wrap these because no code mutates a hydrated `Deployment`; state changes are status-level CAS against persisted rows. | Persistence invariant | Store CAS primitives (`advance_status`, `mark_succeeded`, `mark_failed`) - `src/adapters/stores/deployment_store.rs`; domain gates - `src/domain/deployment.rs` | Same split (domain decides, store persists bookkeeping) | All deployment-row writes live in exactly those three primitives plus creation inserts; timestamps are DB-clock-generated like `requested_at`/`finished_at`/`updated_at`; CHECK constraint enumerates statuses | `tests/deployment_transition.rs::advances_in_order_through_internal_verification` (started_at NULL while Pending, stamped once, preserved after), `compare_and_set_reports_updated_then_stale` | Keep |
| INV-RUN-001 | Runtime endpoints are loopback-only: IPv4 127.0.0.1 with nonzero port; running observations require a validated endpoint. | Value invariant | `validate_loopback_endpoint` used by `ExpectedRuntimeEndpoint::new`/`ContainerObservation::running` - `src/domain/runtime.rs:374-379,90-144` | Same (domain) | SQLite `CHECK (host_address = '127.0.0.1')`, port range checks - `migrations/0007_deployment_release.sql:87-89`; internal health checker rejects non-loopback before connecting (`src/adapters/health_check_internal.rs`) | `src/adapters/health_check_internal.rs::rejects_non_loopback_endpoint_before_connecting`, `rejects_ipv6_loopback_endpoint_before_connecting` | Store-level test asserting the loopback CHECK rejects other addresses |
| INV-RUN-002 | Container port, host port, health path, and expected status are validated value objects (nonzero ports, absolute whitespace-free path starting `/`, status 100–599). | Value invariant | `ContainerPort::new` `HostPort::new` `HealthCheckPath::new` `HealthCheckStatus::new` - `src/domain/runtime.rs:210-306` | Same (domain) | Manifest conversion (`src/adapters/manifest.rs`); SQLite CHECKs (`migrations/0001_application_catalog.sql:37,47`) | `tests/manifest.rs` boundary tests; `tests/domain_values.rs` persisted-value revalidation | Keep |
| INV-RUN-003 | Logical runtime states are closed (`Starting/Running/Stopped/Failed`); unknown Podman states are preserved as typed `Unknown { status }`, absence is explicit `Missing`. | External-boundary invariant | Closed enum - `src/domain/runtime.rs:146-152`; observed enum - `40-50`; mapping - `src/adapters/local_runtime.rs` | Same split (domain owns sets, adapter classifies) | Store hydration errors on unknown logical states, maps observed states to `Unknown` (`runtime_store.rs:368-421,470-483`) | `src/adapters/local_runtime.rs::maps_podman_states_to_explicit_runtime_states`; `src/adapters/stores/runtime_store.rs::loads_a_typed_runtime_state_and_rejects_invalid_persisted_text` | Keep |
| INV-RUN-004 | Retirement is explicit evidence (`removed_at`); a runtime without retirement is logically active, and retired/active cannot contradict removal timestamps. | Entity invariant | `RuntimeRetirement` - `src/domain/runtime.rs:165-169`; store hydration consistency - `runtime_store.rs:368-421` | Same split | None additional | Partially via `loads_a_typed_runtime_state_and_rejects_invalid_persisted_text` | Explicit store test for retired-without-timestamp and active-with-timestamp rejection |
| INV-RUN-005 | Stable external identity is `pneuma-<application>-<deployment-id>` shared by container name, Quadlet file `<base>.container`, and systemd service `<base>.service`. | Cross-object rule | `stable_runtime_name` - `src/domain/runtime.rs:381-385` | Same (domain derives; adapters materialize) | Reconciliation matches containers by this deterministic name | `tests/cli.rs::deploy_writes_boot_enabled_quadlet_unit`; reconcile rematerialization tests in `tests/cli.rs` | Keep |
| INV-EXP-001 | Exposure intent and materialization are distinct: `desired_visibility` is operator intent; `materialization_state` records the confirmed Caddy result; changing intent alone activates nothing. | Entity invariant | `Exposure` separating intent/materialization - `src/domain/exposure.rs:272-300` | Same (domain) | Guarded store transitions pin both fields (`exposure_store.rs:222-319`) | `tests/cli.rs` visibility flows; `tests/reconciliation.rs` | Keep |
| INV-EXP-002 | Materialization evidence combinations are legal only as encoded: Active requires a ConfirmedRoute; Failed/Diverged require a diagnostic; route triple (runtime id, config version, timestamp) is all-or-none. | Entity invariant | `ExposureMaterialization::hydrate` - `src/domain/exposure.rs:235-270`; `ConfirmedRoute::new` - `118-134` | Same (domain) | Store load enforces presence triples before calling hydrate (`exposure_store.rs:96-219`) | `tests/application_specification.rs::rejects_invalid_persisted_exposure_values`; `migrations` CHECK/FK test `exposure_materialization_columns_enforce_state_and_runtime_identity` | Keep |
| INV-EXP-003 | Configuration version is the canonical fragment content (domain + loopback endpoint), never a Release or Deployment ID. | Cross-object rule | Fragment builder + `ExposureConfigurationVersion` - `src/adapters/caddy_exposure.rs`, `src/domain/exposure.rs:92-108` | Same split (adapter computes content, domain types it) | Reconciliation compares fragment contents to detect divergence | `tests/caddy_exposure.rs`; reconcile repair tests in `tests/cli.rs` | Keep |
| INV-REC-001 | Reconciliation loads persisted facts in a short transaction and closes it before observing Podman, Quadlet, and Caddy; decisions consume typed observation inputs. The input type groups facts by authority: `DesiredState` (intent), `PersistedState` (bookkeeping), and observed facts stay in `ReconciliationObservation`. The drift answer is a pure domain function: `decide(input, observation, expectations) -> ReconciliationDecision` classifies InSync, runtime identity repair, rematerialization, internal-route removal, public-route materialization, public-exposure failure records, or manual intervention with no store, filesystem, Podman, systemd, Caddy, clock, or randomness access; the use case only acquires ownership, observes, decides, then executes the decided variant (interrupted-deployment recovery remains use-case compensation orchestration). | Workflow invariant | Input loading - `src/use_cases/reconciliation/load.rs`; pipeline orchestration - `src/use_cases/reconciliation/mod.rs`; input types - `src/domain/reconciliation.rs:16-45`, observation types `84-93`; decision function - `decide` in `src/domain/reconciliation.rs` | Same split (use case orders, domain decides and shapes facts; adapters render canonical expectations) | Per-application kernel lock defers concurrent reconcile | `tests/cli.rs::reconcile_defers_before_external_observation`; `tests/reconciliation.rs::loads_active_snapshot_without_writing_sqlite`; in-file decision matrix in `src/domain/reconciliation.rs` | Public route confirmation compares against boundary-rendered canonical fragments; the unreachable `domain_missing` failure classification was removed because validated `ExposureIntent::Public` guarantees a domain |
| INV-REC-002 | After lock release, an interrupted non-terminal Deployment is recorded failed without external effects; candidate cleanup requires provable persisted+external identity. | Workflow invariant | `recover_interrupted_deployment` - `src/use_cases/reconciliation/recover.rs` | Same (use case) | CAS-guarded writes; identity match before cleanup | `tests/reconciliation.rs::reconcile_marks_an_interrupted_pending_deployment_failed_without_external_effects`, `reconcile_cleans_a_verified_candidate_only_after_unit_identity_is_proven`, `reconcile_reports_manual_intervention_when_a_candidate_identity_cannot_be_proven`, `reconcile_reports_manual_intervention_when_an_interrupted_candidate_has_no_persisted_runtime`, `reconcile_marks_an_interrupted_activation_route_diverged_when_prior_route_is_unproven`, `reconcile_preserves_a_proven_prior_route_when_an_activation_was_interrupted`; repair/rematerialize family in `tests/cli.rs` | Keep |
| INV-REC-003 | `Missing` is an observation, not a tombstone; `removed_at` is reserved for candidate cleanup, retirement, and intentional removal. Reconcile never creates a new Deployment/RuntimeInstance because a container is missing. | Cross-object rule | Decision owner - pure domain policy (`classify_runtime_rematerialization` refuses to invent resources) - `src/domain/reconciliation.rs`; design contract - `docs/design/reconciliation.md` (Invariants 1–2) | Same (domain decides, use case executes) | Store retirement semantics (`INV-RUN-004`) | In-file decision matrix in `src/domain/reconciliation.rs` (rematerialization only for Missing containers of the confirmed identity); reconcile repair tests in `tests/cli.rs` | Keep |
| INV-REC-004 | Every reconciliation recovery/repair action follows the documented contract ("Reconciliation Recovery And Compensation Contract" below): persistence reservation before external effect, explicit confirmation after observation, CAS-guarded persistence, defined partial-failure compensation that is never silent success, and re-runnable idempotent effects. | Workflow invariant | `src/use_cases/reconciliation/recover.rs`, `execute.rs`; store CAS primitives in `runtime_store.rs`/`exposure_store.rs` | Same split (use cases order, adapters persist/execute) | Operation generation fencing (INV-DB-005); kernel lock (INV-WF-007) | Tests listed per action in the contract section below | Keep |
| INV-WF-001 | Persist intent before external effect; persist confirmed completion after observing the effect (deploy intent, start/stop intent, exposure applying/removing). | Workflow invariant | Use-case sequencing - `src/use_cases/application/runtime.rs`, `src/use_cases/exposure/mod.rs`, `deployment/candidate.rs` | Same (use cases) | Guarded store transitions make out-of-order writes stale | `tests/cli.rs::public_visibility_without_a_domain_is_rejected_before_external_effects`; lifecycle idempotency tests | Keep; ordering assertions remain scenario-level |
| INV-WF-002 | No SQLite transaction remains open during Git, OCI, Podman, systemd, Caddy, or HTTP work. | Workflow invariant | Use-case structure (transactions scoped to store calls only) across `src/use_cases/` | Same (use cases) | Writer-lock acquisition inside immediate transactions only (`tests/deployment_create.rs::immediate_transaction_acquires_the_writer_lock_before_reading`) | Proxy coverage only | Dedicated structural/scenario test proving transactions close before external calls (known gap) |
| INV-WF-003 | Public promotion atomically records succeeded Deployment, active Deployment ID, current RuntimeInstance, and active Exposure in one transaction. | Workflow invariant | Promotion transactions - `src/use_cases/deployment/promotion.rs` | Same (use cases own transaction boundaries) | Unique indexes catch partial states (`one_current_runtime_per_application`) | `tests/deployment_promote_internal.rs::replaces_the_previous_current_runtime_atomically`, `promotes_healthy_candidate_idempotently` | Keep |
| INV-WF-004 | A failed candidate never replaces the prior active runtime or public route; cleanup removes only resources proven to belong to that candidate; prior-runtime retirement after promotion is best effort. | Workflow invariant | Cleanup - `src/use_cases/deployment/cleanup.rs`; execute-release compensation paths | Same (use cases) | Promotion atomicity means old route persists until success | `tests/deployment_promote_internal.rs::unhealthy_candidate_fails_without_replacing_current_runtime`; `tests/cli.rs::failed_public_health_restores_previous_fragment_and_keeps_public_intent`, `restores_previous_public_route_when_external_health_fails`; `cleanup_does_not_remove_already_promoted_runtime` | Keep |
| INV-WF-005 | Materialization failure compensates by restoring the previous Caddy fragment; incomplete compensation records `diverged` for manual intervention, never silent success. | Workflow invariant | Exposure change/promotion compensation - `src/use_cases/exposure/mod.rs`, `deployment/promotion.rs` | Same (use cases) | `ExposureOutcome::{Failed,Diverged}` typed outcomes; CAS confirmation | `tests/cli.rs::lost_public_completion_cas_restores_the_fragment_and_is_not_success` | Keep |
| INV-WF-006 | Start/stop are idempotent; repeating visibility requests matching current desired visibility succeed without touching materialization. | Workflow invariant | `src/use_cases/application/runtime.rs`, `src/use_cases/exposure/mod.rs` | Same (use cases) | Typed intent comparisons before effects | `tests/cli.rs::stop_and_start_are_idempotent...`; visibility repeat tests | Keep |
| INV-WF-007 | One live per-application kernel lock serializes deploy/reconcile work; reconcile defers while the lock is held. | Workflow invariant | `src/adapters/application_lock.rs:52-93` + use-case acquisition | Same split (adapter owns flock mechanics, use case acquires) | Lock file never unlinked (stable inode); process death releases lock | `src/adapters/application_lock.rs` serialization/independence test; `tests/cli.rs::reconcile_defers_before_external_observation` | Keep |
| INV-DB-001 | Only one non-terminal Deployment may exist per Application. | Persistence invariant | Partial unique index `one_active_deployment_per_application` - `migrations/0007_deployment_release.sql:73` (originally `0002:43`) | Same | Use case checks before insert; kernel lock serializes attempts | `tests/deployment_create.rs::rejects_a_second_active_deployment`; `tests/cli.rs::a_second_deploy_is_rejected_while_the_first_is_starting` | Keep |
| INV-DB-002 | Releases, Deployments referencing them, and Runtimes referencing those Deployments all belong to the same Application; mismatches are rejected by triggers on insert and update. | Persistence invariant | Triggers - `migrations/0009_deployment_release_application.sql`, `migrations/0010_runtime_deployment_application.sql`; FKs elsewhere | Same | Domain constructors carry application IDs through all stores | `tests/deployment_create.rs::database_rejects_a_release_from_another_application`; `tests/deployment_register_runtime.rs::database_rejects_a_runtime_identity_from_another_application` | Keep |
| INV-DB-003 | A live loopback endpoint is unique while a runtime is not removed; each candidate reserves its port before registration and reservations are consumed/released exactly once. | Persistence invariant | Unique index `active_runtime_endpoint` - `migrations/0007_deployment_release.sql:132`; reservation PK on `port` - `migrations/0012_runtime_port_reservations.sql:2`; allocator immediate transaction - `src/adapters/port_allocator.rs:49-114` | Same | Allocator checks live runtimes UNION pending reservations atomically | `tests/deployment_register_runtime.rs::database_rejects_a_duplicate_active_endpoint`, `identical_retry_is_idempotent_but_conflicting_reuse_is_rejected` | Unit/integration tests for the port allocator itself: range parsing, exhaustion, exclusivity (known gap) |
| INV-DB-004 | Every persistence write racing on state uses compare-and-set; a zero-row update is stale/concurrent state, never success. | Persistence invariant | `advance_status` (`deployment_store.rs:238`), `compare_and_set_desired_runtime_state` (`application_store.rs:233`), runtime CAS (`runtime_store.rs:129-153`), all exposure transitions (`exposure_store.rs:222-319`), `mark_failed` zero-row → explicit error | Same (stores own CAS mechanics) | Operation fencing generation distinguishes ownership epochs | `src/adapters/stores/deployment_store.rs::compare_and_set_reports_updated_then_stale`; `tests/cli.rs::lost_public_completion_cas_restores_the_fragment_and_is_not_success`, `status_does_not_attempt_external_id_cas` | Keep |
| INV-DB-005 | Ownership coordination uses a monotonic per-application generation; taking ownership advances it and replaces the token. | Persistence invariant | `operation_store::take_ownership` upsert `generation = generation + 1 RETURNING` - `src/adapters/stores/operation_store.rs:43-66`; `CHECK (generation > 0)` - `migrations/0015_application_operations.sql:4` | Same | PK on `application_id`; SQLite statement serialization | `src/adapters/stores/operation_store.rs::ownership_replaces_the_token_and_advances_the_generation` | Keep |
| INV-DB-006 | Corrupt or invalid persisted values are conversion errors, never silently mapped to invented defaults; only three documented legacy tolerances exist (`SourceRevision::Legacy`, `DeploymentFailureEvidence::Incomplete`, and the NULL `applications.system_id` of INV-APP-003). | Persistence invariant | All store row mappers (`application_store.rs:132-146,374-540`; `deployment_store.rs:421-520`; `runtime_store.rs:368-483`; `exposure_store.rs:96-219`; `system_store.rs:73-85`) | Same (adapters own encoding; domain validates) | Domain constructors reject at hydration time | `tests/domain_values.rs`; `tests/application_specification.rs::rejects_invalid_persisted_specification/exposure_values`; `preserves_incomplete_historical_failed_evidence` | Keep |
| INV-DB-007 | Migrations are immutable, forward-only, recorded in `schema_migrations`, applied on connection open with foreign keys enabled; upgrades are tested fresh and from the immediately preceding schema. | Persistence invariant | Migration runner - `src/adapters/database.rs:286-345` | Same (adapter) | Backup/restore commands for downgrade recovery | `open_configures_and_migrates_database`, `migration_is_idempotent`, per-step upgrade chain tests incl. backfill assertions in `src/adapters/database.rs` | Keep |
| INV-SRC-001 | Git revisions are peeled to immutable commits via `<rev>^{commit}` with `--verify --end-of-options`; failures are classified (repository/auth/branch/commit), never collapsed. | External-boundary invariant | `src/adapters/git_source.rs:221-299` | Same (adapter converts to safe domain types: `CommitSha`) | Domain `CommitSha` re-validates 40-char lowercase hex (`src/domain/git.rs:164`) | `tests/git_source.rs::commit_sha_accepts_full_hex_sha`, `rejects_invalid_identifier`; resolution tests | Keep |
| INV-SRC-002 | Checkouts are isolated detached clones (`--no-hardlinks`, destination must not pre-exist, failed checkouts removed, reuse requires clean tree at same HEAD). | External-boundary invariant | `src/adapters/git_source.rs:310-476` | Same (adapter) | Import removes checkout after persistence | `tests/git_source.rs` clone/reuse tests; `tests/cli.rs` import cleanup tests | Keep |
| INV-SRC-003 | Pulled images are digest-verified against the declared artifact; mismatch is a hard error; only canonical lowercase sha256 digests accepted. | External-boundary invariant | `src/adapters/oci_image.rs:142-154,281-297` | Same (adapter verifies; domain types identity) | `OciArtifact` validation before pull (`INV-REL-001`) | `src/adapters/oci_image.rs` tests; `tests/deployment_from_oci.rs::verified_digest_deployment...`; `tests/oci_image.rs` (3 ignored - SKIP: need rootless Podman host) | Run ignored registry tests on a configured rootless Podman host |
| INV-SRC-004 | All systemd control is `--user`; unit absence (exit 4) maps to Missing; unit removal is idempotent; boot-start comes from `WantedBy=default.target`, never `systemctl enable`. | External-boundary invariant | `src/adapters/systemd_quadlet.rs:128,162-262` | Same (adapter) | Quadlet content asserted in deploy scenarios | `tests/cli.rs::deploy_writes_boot_enabled_quadlet_unit`; lifecycle/remove-container cycles | Keep |
| INV-EXT-001 | Internal health checks connect only to loopback endpoints, use bounded retries (5 attempts × 2 s timeout × 500 ms interval), read a capped status line, and classify timeout vs unreachable. | External-boundary invariant | `src/adapters/health_check_internal.rs:10-12,76-194` | Same (adapter; bounds fixed in production) | Domain endpoint loopback validation upstream (`INV-RUN-001`) | 8 in-file unit tests incl. `rejects_non_loopback_endpoint_before_connecting` | Keep |
| INV-EXT-002 | External health pins the configured domain to loopback via `curl --resolve <domain>:443:127.0.0.1` with proxy bypass, bounded attempt window, and long bounded ACME retry; status must equal expected. | External-boundary invariant | `src/adapters/health_check_external.rs:61-117` | Same (adapter) | Health spec validated at import (`INV-RUN-002`) | `tests/cli.rs` asserts `--resolve` usage (~line 536) and public-health failure paths | Isolated unit tests for the external checker's timeout/retry semantics (known gap) |
| INV-EXT-003 | Managed Caddy fragments live at `<application-id>.caddy`, are imported by the main Caddyfile, and untrusted fragment coordinates (path traversal/unexpected names) are rejected before external work. | External-boundary invariant | `src/adapters/caddy_exposure.rs` | Same (adapter) | Exposure store guards route identity to application | `tests/caddy_exposure.rs::rejects_untrusted_fragment_coordinates_before_external_work` (+12 file tests) | Keep |
| INV-EXT-004 | Port allocation respects the configured `PNEUMA_RUNTIME_PORT_RANGE`; malformed ranges (zero, inverted, non-numeric bounds) are rejected. | External-boundary invariant | `src/adapters/port_allocator.rs:10-11,116-130` | Same (adapter) | Reservation exclusivity in SQLite (`INV-DB-003`) | None directly | Allocator tests covering boundary values of the configured range (known gap) |
| INV-CI-001 | The restricted SSH dispatcher permits only `version` and `deploy <application> <branch-or-tag>`; both arguments are validated with domain rules; injection attempts are rejected. | Entity invariant | `parse_ci_command` - `src/use_cases/ci/mod.rs` (rules in library, correct owner); `src/cli/ci.rs` only plumbs `SSH_ORIGINAL_COMMAND` | Same | Dispatcher key reaches only this restricted path (security model) | 13 in-file unit tests incl. `parse_injection_attempts_rejected`, `valid/invalid_application_names` | Keep |

## Primitive And Value Object Audit

Recorded by consolidation iteration 02 so every recurring primitive carries an
explicit classification instead of an accidental one. Categories:

- **Value Object** - validated or otherwise restricted construction; the type,
  not call sites, guarantees the rule.
- **Intentional Primitive** - stays a primitive on purpose; its one rule is
  enforced once where the value is produced.
- **Boundary-only Type** - exists only at an external edge to convert input
  into domain-safe types.
- **Read-model Primitive** - presentation/projection text that never re-enters
  domain rules.

| Candidate | Classification | Decision and owner |
|---|---|---|
| `ApplicationName` | Value Object | Catalog-name rule owned by `ApplicationName::new` (`src/domain/application.rs`) over the shared predicate `is_valid_catalog_name` (`src/domain/identity.rs:144`). |
| `SystemName` | Value Object | Same shared rule owned by `SystemName::new` (`src/domain/system.rs`). |
| `SystemId`, `ApplicationId`, `ReleaseId`, `DeploymentId`, `RuntimeInstanceId` | Value Object | Newtypes for semantic distinction and argument-mixup prevention (`src/domain/identity.rs`; see the non-interchangeability test in `src/domain/runtime.rs`). By explicit decision they impose no format rule so legacy SQLite text round-trips unchanged; construction stays via `From` impls and APIs must not widen back to raw `String`. |
| OCI repository | Value Object | `OciRepository` owns the repository grammar (`src/domain/release.rs`); consumed through `OciArtifact` and `DeliverySpecification`, never re-parsed downstream. |
| Image digest | Intentional Primitive | No standalone type: the sha256 digest is validated exactly once inside `OciArtifact::parse` (`is_sha256_digest`, `src/domain/release.rs`) and has no behavior or independent lifecycle; adapters only compare it against the artifact (`src/adapters/oci_image.rs`). Revisit only if a digest ever flows separately from its artifact. |
| Healthcheck path | Value Object | `HealthCheckPath` requires an absolute whitespace-free path starting `/` (`src/domain/runtime.rs`). |
| Expected HTTP status | Value Object | `HealthCheckStatus` accepts only 100–599 (`src/domain/runtime.rs`). |
| Container port | Value Object | `ContainerPort` rejects zero (`src/domain/runtime.rs`). |
| Host port | Value Object | `HostPort` rejects zero (`src/domain/runtime.rs`). |
| Domain/hostname | Value Object | `DomainName` owns the domain grammar (`src/domain/exposure.rs`). |
| Source revision | Value Object + Read-model Primitive | New revisions must be a validated `CommitSha` (40-char lowercase hex, `src/domain/git.rs`). Historical rows hydrate through `SourceRevision::Legacy` (`src/domain/deployment.rs`), the documented legacy tolerance of INV-DB-006 that is never accepted as a new commit value. |
| Manifest path | Value Object | `RelativeManifestPath` rejects empty, absolute, root, prefix, and parent components (`src/domain/git.rs`); used both by the manifest loader and persisted sources. |
| Specification version | Intentional Primitive | `schema_version: u32` is compared once against `SUPPORTED_SCHEMA_VERSION` at the manifest boundary (`src/adapters/manifest.rs`); equality-only semantics give a dedicated type nothing to own. The validated copy travels as `ImportSpecification.schema_version` (`src/domain/manifest.rs`), which is TOML-free. |

No audited candidate qualifies as a Boundary-only Type today: every
external-input rule already lands in a domain-owned validated type at the
manifest, Git, or OCI boundary. Exit criterion met: every candidate listed by
iteration 02 has an explicit decision.

## Struct Role Classification

Recorded by consolidation iteration 12 so every public struct and enum carries
an explicit role instead of an accidental one. Categories:

- **Entity** - durable identity plus lifecycle; the invariant authority for its
  aggregate.
- **Value Object** - validated or restricted construction (see the audit above).
- **Domain state** - closed set, decision output, or evidence bundle owned by
  the domain.
- **Read model** - query/projection shape for display; never carries invariant
  authority.
- **Use-case input/output** - workflow input, output, progress, or error type.
- **Adapter DTO** - external representation exchanged with one adapter.
- **Persistence row** - private store-level row mapping.

| Type | Role | Notes |
|---|---|---|
| `Application`, `System`, `Release`, `Deployment`, `RuntimeInstance`, `Exposure` (`src/domain/`) | Entity | The only invariant authorities for their aggregates. Every mutation path loads one of these (e.g. `load_application_by_name`) before deciding; no use case decides from a projection. |
| `ApplicationSummary` (`src/domain/application.rs`) | Read model | Catalog projection hydrated by `application_store` and returned only by list/import/remote-import/show flows. Marked in code as carrying no invariant authority. |
| `DeploymentHistory` (`src/domain/deployment.rs`) | Read model | Deployment + Release + active marker for `app deployments`. Transitions/promotions load persisted status through CAS primitives, never this view. |
| `SystemDetails` (`src/use_cases/system/show.rs`) | Read model | System entity plus its catalog summaries for one show flow. |
| Value Objects and intentional primitives | Value Object | All rows of the "Primitive And Value Object Audit" above. |
| `DesiredRuntimeState`, `DeploymentStatus`, `DeploymentLifecycle`, `DeploymentEvent`, `DeploymentType`, `RuntimeState`, `ObservedRuntimeState`, `Visibility`, `ExposureIntent`, `ExposureOutcome`, `ExposureMaterializationState`, `RepositoryKind`, `DeliveryType` | Domain state | Closed sets owned by the domain; adapters classify into them, stores persist exactly their values. |
| `DeploymentFailure`, `DeploymentFailureEvidence`, `SourceRevision`, `ConfirmedRoute`, `ExposureDiagnostic`, `ExposureMaterialization`, `PromotionTarget`, `PromotedCandidate`, `PromotionCandidateRejection`, `RollbackTarget`, `RuntimeRetirement`, `RuntimeRegistration`, `PreviousRuntime`, `ExpectedRuntimeEndpoint`, `ActiveRuntime`, `ReconciliationInput` with its `DesiredState`/`PersistedState` authority groups, observation enums (`src/domain/reconciliation.rs`, `ContainerObservation`) | Domain state / evidence bundle | Pure facts and decision outputs; produced by hydration or observation, consumed by domain gates and use cases without infrastructure. |
| `ImportSpecification`, `CiCommand` | Use-case input | Boundary-validated inputs; TOML-free and effect-free. |
| `ApplicationDeploymentSpecification` (`src/domain/application.rs`) | Use-case input | Persisted fact bundle loaded whole for deploy/promote/reconciliation; not an entity — intent writes stay ID-keyed in store primitives. |
| Use-case outputs and errors (`DeploymentResult`, `PublicActivationOutput/Input`, `CandidateStartInput/StartedCandidate`, `CandidateResources`, `ProgressReporter`, `DeploymentStep/Progress`, `RuntimeObservation`, all `*Error` enums) | Use-case input/output | Owned by the orchestrating flow; private unless a genuine library API. |
| `ManifestDocument` and section structs (`src/adapters/manifest.rs`), `PulledImage`, `ExternalHealthCheck`, `MaterializedCaddyFragment`, `RemovedCaddyFragment`, `CaddyFilesystemAction`, `CaddyCommandOutput`, `ContainerCommandOutput`, `HealthCheckResult/Failure` | Adapter DTO | Private or adapter-scoped external representations; converted once at the boundary into domain types. |
| `RawDeployment` (`src/adapters/stores/deployment_store.rs`), `OperationOwnership` (`operation_store.rs`), `PersistenceOutcome` | Persistence row / store primitive | Store-private encoding; never escapes the stores layer. |
| CLI types (`Cli`, command enums, `Invocation` in `src/cli/args.rs`) | Use-case input (CLI edge) | Converted to use-case inputs; hold no domain rules. |
| `CliError`, `CliErrorClass`, render functions (`src/cli/error.rs`, `src/cli/output.rs`) | Presentation (CLI edge) | Classify failures into usage/not-found/conflict/external/failure exit codes and render command results as strings; preserve the source error chain and hold no domain decisions. |

Exit criterion met: no code path consumes a read model where an entity is
required — mutation and transition flows load entities or persisted status via
store primitives (`application_lookup`, reconciliation reads, promote/rollback
gates), while `ApplicationSummary`, `DeploymentHistory`, and `SystemDetails`
are returned only by query/display flows.

## Reconciliation Recovery And Compensation Contract

Recorded by consolidation iteration 17 so every repair and recovery action has
an explicit rule and test (INV-REC-004). Facts shared by all paths:

- **Ownership** - the per-application kernel lock plus monotonic operation
  generation serialize all work (INV-WF-007, INV-DB-005); every persistence
  write inside a path is compare-and-set (INV-DB-004).
- **Retry** - every path is safe to re-run: decisions are re-derived from fresh
  observation on each reconcile, reservations are consumed once, and effects
  are either naturally idempotent or guarded so a repeat converges.
- **Transactions** - never held across external effects (INV-WF-002).

Per action (rule owner first, then test):

1. **Interrupted Pending deployment** (`recover.rs`, Pending arm;
   `tests/reconciliation.rs::reconcile_marks_an_interrupted_pending_deployment_failed_without_external_effects`).
   Pré-condição: lock released with a non-terminal Pending Deployment. Efeito:
   none external. Confirmação: not applicable. Persistência: CAS
   `fail_deployment` records Failed with code `operation_interrupted`. Falha
   parcial: stale CAS surfaces as an error, next reconcile retries. Retry/
   idempotência: terminal after one success; later runs skip via the
   non-terminal gate. Compensação: none needed — nothing was materialized.

2. **Interrupted candidate (Starting/Verifying)** (`recover.rs`;
   `tests/reconciliation.rs::reconcile_cleans_a_verified_candidate_only_after_unit_identity_is_proven`,
   `reconcile_reports_manual_intervention_when_a_candidate_identity_cannot_be_proven`,
   `reconcile_reports_manual_intervention_when_an_interrupted_candidate_has_no_persisted_runtime`).
   Pré-condição: non-terminal candidate plus its persisted runtime row; without
   that row cleanup ownership cannot be proven ⇒ ManualIntervention. Efeito:
   stop/remove proven unit, remove proven container, mark runtime missing,
   release port. Confirmação: unit bytes equal the canonical unit AND full
   container identity match (id, name, image reference, application/digest
   labels, endpoint) before any removal; unprovable identity ⇒ ManualIntervention
   with zero cleanup. Persistência: failure recorded first, retirement after the
   external effects (`mark_starting_runtime_missing` CAS). Falha parcial:
   cleanup errors abort as NotConverged leaving partial resources for the next
   run (each step individually idempotent). Ownership: use case orchestrates;
   `cleanup_failed_candidate` owns adapter effects. Compensação: deliberately
   none — nothing unproven is ever removed.

3. **Interrupted activation (Activating)** (`recover.rs`;
   `tests/reconciliation.rs::reconcile_marks_an_interrupted_activation_route_diverged_when_prior_route_is_unproven`,
   `reconcile_preserves_a_proven_prior_route_when_an_activation_was_interrupted`).
   Pré-condição: non-terminal Activating Deployment. Efeito: none external —
   the prior route is never touched. Confirmação: prior canonical route proven
   only when the confirmed route matches the active runtime AND the on-disk
   fragment equals the recorded configuration version. Persistência: deployment
   marked failed; exposure failure recorded from the Applying reservation —
   Failed when the prior route is proven preserved, Diverged otherwise; stale
   exposure ⇒ ManualIntervention. Retry: re-records only while the reservation
   still matches. Compensação: none required because no new effect happened.

4. **Runtime identity repair** (`execute.rs::confirm_runtime_identity`;
   `src/adapters/stores/runtime_store.rs::identity_cas_is_stale_unless_the_recorded_container_id_matches`;
   `tests/cli.rs::reconcile_repairs_a_confirmed_quadlet_container_recreation`).
   Pré-condição: pure decision proved a recreated container's full identity.
   Efeito: none external. Persistência: single CAS swap of
   `external_runtime_id`. Falha parcial: stale ⇒ NotConverged, retried by the
   next reconcile. Idempotência: repeating with the same observation converges.

5. **Runtime rematerialization** (`execute.rs::rematerialize_runtime`;
   `tests/cli.rs::reconcile_rematerializes_a_missing_quadlet_and_container`,
   `reconcile_restarts_a_canonical_quadlet_after_its_container_is_removed`,
   `reconcile_reports_manual_intervention_for_a_divergent_recreated_container`).
   Pré-condição: decision proved container Missing (and optionally divergent
   Quadlet bytes) with a startable generated unit. Efeito: canonical unit write
   + daemon-reload only when needed, then systemd start. Confirmação: full
   container identity re-observed and matched, then internal health check.
   Persistência: identity CAS confirm strictly after healthy observation.
   Falha parcial: absent/divergent rematerialization ⇒ Failed/ManualIntervention
   without persistence; health failure ⇒ Failed; stale CAS ⇒ NotConverged.
   Idempotência: canonical-byte writes and systemd start are idempotent.
   Compensação: none automatic — remaining drift is re-decided next run.

6. **Internal route removal** (`execute.rs::remove_internal_route`;
   `tests/cli.rs::reconcile_removes_an_internal_caddy_fragment`,
   `lost_removal_completion_cas_restores_the_fragment_and_records_failure_during_reconcile`).
   Pré-condição: decision RemoveInternalRoute carrying the persisted snapshot
   state. Persistência-first: CAS reservation to Removing before any effect.
   Efeito: managed fragment removal + Caddy validate/reload. Confirmação/
   persistência: atomic completion CAS Removing→NotMaterialized clearing the
   route triple. Falha parcial: removal error ⇒ failure record flagged by
   `recovery_failed()`; lost completion CAS ⇒ restore removed fragment, then
   record `exposure_changed` (Diverged if restoration also failed). Idempotência:
   removing an already-absent fragment is decided away before any effect.
   Ownership: use case orders reserve→effect→confirm; adapter owns files/Caddy.
   Compensação: `restore_removed_caddy_fragment`.

7. **Public route materialization** (`execute.rs::materialize_public_route`;
   `tests/cli.rs::reconcile_repairs_a_missing_public_caddy_fragment_with_configured_caddyfile`,
   `reconcile_records_failed_public_exposure_when_external_health_cannot_confirm_it`).
   Pré-condição: decision MaterializePublicRoute for public intent with an
   active runtime. Persistência-first: CAS reservation to Applying. Efeito:
   canonical fragment materialize + Caddy validate/reload, then external health
   check pinned to loopback. Confirmação/persistência: completion CAS
   Applying→Active writing the route triple. Falha parcial: materialization
   error ⇒ failure record; health failure ⇒ restore previous fragment + record;
   lost completion CAS ⇒ restore + record. Idempotência: canonical bytes are
   deterministic. Compensação: `restore_materialized_caddy_fragment`; incomplete
   compensation records Diverged, never silent success (INV-WF-005).

8. **Public exposure failure record**
   (`execute.rs::record_public_exposure_failure`; unhealthy/missing-runtime
   scenarios in `tests/cli.rs`). Pré-condição: pure decision classified
   RuntimeMissing/RuntimeNotHealthy carrying exact persisted codes. Efeito:
   none external. Persistência: single CAS diagnostic record valid only while
   the expected reservation is current; stale ⇒ NotConverged surfaced as error.
   Idempotência: bounded by the reservation; a stale record defers to whatever
   changed the state.

## Known Coverage Gaps

Recorded here so later iterations can schedule them; none blocks this
inventory:

1. No direct test proves transactions close before external effects
   (INV-WF-002); coverage is structural/proxy only.
2. Rollback happy path (new Deployment executed from historical provenance,
   INV-DEP-005) has no E2E test; only guards and provenance selection are
   covered.
3. `port_allocator.rs` and `systemd_quadlet.rs` have no dedicated in-file
   tests; they are exercised indirectly through CLI fakes (INV-DB-003,
   INV-EXT-004, INV-SRC-004).
4. The external health checker has no isolated timeout/retry tests
   (INV-EXT-002).
5. Three `tests/oci_image.rs` tests are ignored environment tests requiring a
   configured rootless Podman host; they must be recorded PASS/SKIP with
   reason on such a host, never assumed green (INV-REL-001, INV-SRC-003).

## Sources Reviewed

`docs/architecture/architecture.md`, `docs/architecture/data-model.md`,
ADRs `0001`–`0007`, `docs/design/reconciliation.md`, `src/domain/`,
`src/use_cases/`, `src/adapters/`, `migrations/0001`–`0015`, and the test
suite (`tests/*.rs` plus in-file unit tests).
