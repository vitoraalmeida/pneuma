# Pneuma Invariant Inventory

**Status:** living document - the authoritative inventory of architectural
invariants, produced by consolidation iteration 01.

Every relevant architectural rule is listed here with its category, its current
owner in code, its desired owner, its secondary defense line, and the tests that
prove it. No rule of the architecture may remain only implicit; if a rule is
missing from this table, either add it or record why it does not apply.

## What Is An Invariant

An invariant is a property that must hold true at every point in time — across
every request, crash, restart, concurrent operation, and external failure — for
the system to remain correct. Invariants are not step-by-step behavior; they are
the guarantees behavior must never break. For Pneuma, typical invariants are:

- structural rules that make invalid values unrepresentable (a port is nonzero,
  a digest is lowercase sha256);
- lifecycle rules that only certain state changes may ever happen (a failed
  candidate never replaces the active runtime);
- coordination rules that concurrent or repeated operations cannot corrupt
  persisted facts (per-Application locks and targeted conditional writes);
- boundary rules about trusting outside input (unknown Podman states are
  preserved as `Unknown`, never adopted as known-safe values).

Every invariant names exactly one owner — the layer responsible for enforcing
it — plus a secondary defense line and tests. When changing code, find the
invariants you touch here first; when adding code, classify every rule before
moving it.

## How to Read This Table

Categories follow the consolidation classification:

- **Value invariant** - determined from a single value in isolation. The rule
  belongs to a validated domain type constructed once at the boundary, so an
  invalid value cannot exist afterward. Example: INV-RUN-002 (nonzero ports,
  bounded health status) owned by `ContainerPort::new`/`HealthCheckStatus::new`.
- **Entity invariant** - determined by an entity together with the requested
  operation; whether the change is legal depends on the entity's current state,
  not just the new value. Owned by entity behavior or a pure domain function.
  Example: INV-DEP-001 (only legal status transitions) owned by
  `DeploymentStatus::transition`.
- **Cross-object rule** - depends on more than one domain object being
  consistent with each other. Owned by a pure domain function comparing both,
  or enforced with persistence support when races are possible. Example:
  INV-REL-002 (an artifact may only come from the application's configured
  repository) owned by `DeliverySpecification::permits`.
- **Persistence invariant** - protects stored facts against races, duplication,
  or corruption; several writers could otherwise violate it. Owned by SQLite
  constraints/triggers plus adapter compare-and-set mechanics, with use-case
  cooperation. Example: INV-DB-004 (zero-row CAS update is conflict, never
  success).
- **Workflow invariant** - ordering of persistence and external effects across
  steps of one operation (what must happen before what). Owned by use cases.
  Example: INV-WF-001 (persist intent before the external effect, confirm after
  observing it).
- **External-boundary invariant** - data or effects arriving from systems
  Pneuma does not control (Git, OCI registries, Podman, systemd, Caddy, HTTP);
  the rule is that untrusted or open-ended input is converted or classified
  once at the edge into safe domain types. Example: INV-REC-005 (unknown
  external states stay explicitly unknown).

"Desired owner" differs from "current owner" only where the inventory found an
ownership gap; identical entries mean the rule already lives where it belongs.

Per-type classifications — which recurring primitives are Value Objects versus
intentional primitives, and which role each struct plays (entity, read model,
domain state, use-case input/output, adapter DTO, persistence row) — are kept
as comments on the types themselves in `src/`, so the classification sits next
to the code it describes instead of drifting here.

| ID | Rule | Category | Current owner | Desired owner | Secondary defense | Current test | Desired test |
|---|---|---|---|---|---|---|---|
| INV-SYS-001 | System names satisfy the shared catalog name rule: 1–63 chars, lowercase ASCII letters/digits/hyphens, alphanumeric first and last. | Value invariant | `SystemName::new` - `src/domain/system.rs:18`; shared predicate `is_valid_catalog_name` - `src/domain/identity.rs:144` | Same (domain) | Manifest import validation (`src/adapters/manifest.rs`); CLI uses `SystemName::new` (`src/cli/system.rs:17,41`) | `tests/manifest.rs::accepts_name_domain_and_status_boundaries`, `rejects_overlong_names_and_domains`; `src/use_cases/ci/mod.rs` name tables; in-file boundary tests `accepts_catalog_names_within_the_shared_rule`, `rejects_names_outside_the_shared_rule` (`src/domain/system.rs`) | Keep |
| INV-APP-001 | Application names satisfy the same catalog name rule as Systems. | Value invariant | `ApplicationName::new` - `src/domain/application.rs:45`, predicate `src/domain/identity.rs:144` | Same (domain) | Manifest validation (`src/adapters/manifest.rs`); CI command parsing reuses it (`src/use_cases/ci/mod.rs`) | `src/use_cases/ci/mod.rs::parse_deploy_reuses_the_domain_application_rule`, `valid/invalid_application_names`; `tests/manifest.rs`; in-file boundary tests `accepts_catalog_names_within_the_shared_rule`, `rejects_names_outside_the_shared_rule` (`src/domain/application.rs`) | Keep |
| INV-APP-002 | Application persisted state has exactly three mutation paths: the immutable identity field (`system_id`) is written once at import and never updated; runtime intent changes under the Application lock or through activation; activation writes `active_deployment_id` paired with running intent only for a succeeded Deployment belonging to that Application. No code mutates a hydrated `Application`; `request_start`/`request_stop`/`activate` entity methods are deliberately absent because intent writes are ID-keyed store operations, not field mutations of a loaded entity (same rationale as INV-DEP-006). | Cross-object rule | Import insert (`insert_application`), lock-protected intent primitive (`set_desired_runtime_state`), guarded activation primitive (`activate_deployment`) - all in `src/adapters/stores/application_store.rs`; eligibility decided by domain (`DeploymentStatus::transition(Activated)` gates both promote use cases) | Same split (domain decides eligibility, store enforces the guarded write) | FK `applications.active_deployment_id REFERENCES deployments(id)` - `migrations/0007_deployment_release.sql:136`; promotion transactions atomic (INV-WF-003); conditional writes surface zero rows as conflicts (INV-DB-004) | `tests/application_specification.rs::activates_a_succeeded_deployment_of_the_application_with_running_intent`, `rejects_foreign_or_unsucceeded_activation_without_changing_application_state`; promote flows via `tests/deployment_promote_internal.rs` | Keep |
| INV-APP-003 | `Application.system_id: SystemId` is required: every import resolves or creates exactly one System (`insert_application` takes `&SystemId`, so no code path can insert NULL), and hydration rejects a row without a System instead of tolerating it. There are no legacy tolerances left in the domain. | Persistence invariant | Import insert requires a resolved System (`insert_application`); required-system hydration (`map_application_row`) - both in `src/adapters/stores/application_store.rs` | Same split (store owns encoding; domain validates) | FK `applications.system_id REFERENCES systems(id)`; the column remains physically nullable only until the baseline-schema checkpoint replaces it | `tests/application_list.rs::rejects_legacy_applications_without_a_system`; `tests/application_import.rs` (every import carries one System) | Keep |
| INV-APP-004 | The manifest `schema_version` is an import-boundary check only: the manifest adapter rejects anything but the supported version, and no schema-version copy is persisted on the Application or carried by the domain. | External-boundary invariant | Schema gate in `import_specification` - `src/adapters/manifest.rs`; `insert_application` persists no version | Adapter boundary; domain carries no version field | Import use case refuses to proceed on parse error | `tests/manifest.rs::rejects_an_unsupported_schema_version`; `tests/application_import.rs` (no version assertion on the summary) | Keep |
| INV-MAN-001 | Manifests must declare schema version 3 exactly; unknown TOML fields are rejected. | External-boundary invariant | Schema gate in `import_specification` plus private serde deny-unknown-fields document DTO - `src/adapters/manifest.rs` | Adapter boundary (`src/adapters/manifest.rs`); domain consumes the validated `ImportSpecification` | Import use case refuses to proceed on parse error | `tests/manifest.rs::rejects_unknown_fields_and_schema_versions` (and related) | Keep; add negative test for future schema version 4 rejection |
| INV-MAN-002 | Delivery is OCI-only; `[delivery].image` is a repository without tags/digests or surrounding whitespace. | External-boundary invariant | `OciRepository::new` via `import_specification` - `src/adapters/manifest.rs`; repository grammar - `src/domain/release.rs:144-182` | Same (domain) | Delivery spec persisted with `CHECK (delivery_type IN ('oci'))` - `migrations/0011_application_delivery_specs.sql:3` | `tests/manifest.rs`; `tests/release_create.rs::rejects_invalid_artifact_identity` | Keep |
| INV-MAN-003 | Public default visibility requires a valid domain; internal may omit it. | Cross-object rule | `ExposureIntent::new` - `src/domain/exposure.rs:41-52` | Same (domain) | SQLite `CHECK (desired_visibility = 'internal' OR domain IS NOT NULL)` - `migrations/0001_application_catalog.sql:60` | `tests/manifest.rs::requires_a_domain_for_public_exposure`, `allows_internal_exposure_without_a_domain` | Keep |
| INV-REL-001 | A Release artifact identity is always `repository@sha256:<64 lowercase hex>`; mutable tags never become artifacts. | Value invariant | `OciArtifact::parse/new` - `src/domain/release.rs:24-44` | Same (domain) | Store re-validates persisted reference and cross-checks stored columns (`release_store.rs:177-189`); pull-time digest verification (`src/adapters/oci_image.rs:142-154`) | `src/adapters/oci_image.rs` parse/pull tests (`pull_image_pulls_the_pinned_reference_and_confirms_the_digest`, `pull_image_refuses_a_digest_mismatch_invalid_output_and_failed_pulls`, `resolve_image_digest_builds_the_tagged_reference_and_normalizes_the_answer`); `tests/deployment_from_oci.rs::reject_unpinned_reference_at_validation_boundary`; `tests/release_create.rs::rejects_invalid_artifact_identity`; ignored registry tests in `tests/oci_image.rs` (SKIP: no rootless Podman host) | Keep; run `tests/oci_image.rs` on a configured rootless Podman host |
| INV-REL-002 | An Application permits only the OCI repository recorded from its manifest; foreign repositories are rejected before any pull. | Cross-object rule | `DeliverySpecification::permits(&OciArtifact)` - `src/domain/release.rs`; use case applies it before any effect - `src/use_cases/deployment/deploy.rs` | Same split (domain decides permission, use case orders effects) | DB ownership trigger prevents mismatched Release rows after creation (`migrations/0009_deployment_release_application.sql`) | In-file `src/domain/release.rs::permits_the_exact_configured_repository`, `rejects_foreign_and_prefix_repositories`; `tests/deployment_from_oci.rs::repository_not_allowed_before_pull` | Keep |
| INV-REL-003 | `(application_id, image_digest)` uniquely identifies a Release; duplicates are reused, never re-created. | Persistence invariant | Unique index - `migrations/0006_releases.sql`; reuse logic - `src/use_cases/release/mod.rs` | Same | Domain `OciArtifact` equality | `tests/release_create.rs::creates_and_reuses_a_release_from_one_validated_artifact`; `tests/deployment_create.rs::reuses_a_release_for_a_later_deployment_attempt` | Keep |
| INV-DEP-001 | The only legal Deployment transitions are Pending→Starting→Verifying→Activating→Succeeded plus Verifying→Succeeded (internal promotion), with every non-terminal state allowed to fail into Failed; Succeeded/Failed are terminal. | Entity invariant | `DeploymentStatus::transition/is_terminal/can_fail` over `DeploymentEvent` - `src/domain/deployment.rs:181-249` | Same (domain is the single authority) | Store CAS primitives persist only the loaded→domain-approved pair (`advance_status`, `mark_succeeded`, `mark_failed` - `src/adapters/stores/deployment_store.rs`); CHECK constraint enumerates statuses (`migrations/0007_deployment_release.sql:21`) | Full state×event matrix in `src/domain/deployment.rs` tests; `tests/deployment_transition.rs` (all 6 tests incl. `rejects_skipped_and_repeated_transitions_without_changing_state`, `terminal_and_missing_deployments_cannot_enter_the_flow`) | Keep |
| INV-DEP-002 | A newly recorded failure requires trimmed non-empty code, message, timestamp, and a non-terminal stage. | Entity invariant | `DeploymentFailure::validate_details/new` - `src/domain/deployment.rs:82-114` | Same (domain) | Hydration matrix rejects terminal evidence on non-terminal rows (`deployment_store.rs:465-520`) | `tests/deployment_transition.rs::rejects_incomplete_failure_details_without_changing_state`; in-file `src/domain/deployment.rs::failures_require_trimmed_details_and_a_nonterminal_stage`, `failure_construction_rejects_a_missing_timestamp_and_keeps_valid_evidence`; `src/adapters/stores/deployment_store.rs::rejects_terminal_and_nonterminal_evidence_mismatches` | Keep |
| INV-DEP-003 | A failed Deployment carries complete typed evidence: a `DeploymentFailureCode`, the non-terminal stage it failed from, a trimmed message, and the terminal timestamp. Hydration rejects incomplete or unknown-code failed rows instead of tolerating them. | Persistence invariant | `lifecycle_from_values` and `deployment_failure_code_from_value` (the single persisted-string conversion) - `src/adapters/stores/deployment_store.rs` | Adapter validates at hydration; domain type makes incomplete evidence unrepresentable | SQL evidence checks remain a defense line for the baseline schema | `src/adapters/stores/deployment_store.rs::hydrates_complete_failed_evidence`, `rejects_incomplete_historical_failed_evidence`, `rejects_unknown_failure_code_text`; `tests/deployment_list.rs` | Keep |
| INV-DEP-004 | Promotion requires logical state Starting, observed Podman Running, no retirement; an already-Succeeded deployment returns the confirmed promotion idempotently. | Entity invariant | `PromotionTarget::validate_promotion_candidate/completed_promotion` - `src/domain/deployment.rs:246-278` | Same (domain decides; use case executes) | Store promotion transaction is atomic and guarded by CAS | `tests/deployment_promote_internal.rs::promotes_healthy_candidate_idempotently`, `replaces_the_previous_current_runtime_atomically` | Keep |
| INV-DEP-005 | Rollback creates a new `rollback` Deployment for the most recent succeeded non-active Release and never edits prior history. | Entity invariant | `RollbackTarget` selection - `src/domain/deployment.rs:280-285`; orchestration - `src/use_cases/deployment/rollback.rs` | Same split (domain selects, use case orchestrates) | Insert-only deployment history; unique non-terminal index blocks concurrent attempts | `src/use_cases/deployment/rollback.rs::selects_provenance_from_the_historical_deployment`; guard tests in `tests/deployment_rollback.rs`; happy-path E2E in `tests/deployment_execute_release.rs::rollback_executes_a_new_deployment_from_historical_provenance` (insert-only history proven; new rollback Deployment executes and activates) | Keep |
| INV-DEP-006 | Every semantically relevant Deployment mutation routes through a domain operation: event transitions through `DeploymentStatus::transition`, failures through `can_fail()` + `DeploymentFailure::validate_details`, activation confirmation through the `transition(Activated)` gate. The remaining writes are deliberately procedural persistence bookkeeping: insert-only creation of Pending rows, terminal timestamp/evidence writes inside the CAS primitives, and first-start timestamping (`started_at` stamped exactly once when leaving Pending inside `advance_status`). No entity methods wrap these because no code mutates a hydrated `Deployment`; state changes are status-level CAS against persisted rows. | Persistence invariant | Store CAS primitives (`advance_status`, `mark_succeeded`, `mark_failed`) - `src/adapters/stores/deployment_store.rs`; domain gates - `src/domain/deployment.rs` | Same split (domain decides, store persists bookkeeping) | All deployment-row writes live in exactly those three primitives plus creation inserts; timestamps are DB-clock-generated like `requested_at`/`finished_at`/`updated_at`; CHECK constraint enumerates statuses | `tests/deployment_transition.rs::advances_in_order_through_internal_verification` (started_at NULL while Pending, stamped once, preserved after), `compare_and_set_reports_updated_then_stale` | Keep |
| INV-RUN-001 | Runtime endpoints are loopback-only: IPv4 127.0.0.1 with nonzero port; running observations require a validated endpoint. | Value invariant | `validate_loopback_endpoint` used by `ExpectedRuntimeEndpoint::new`/`ContainerObservation::running` - `src/domain/runtime.rs:374-379,90-144` | Same (domain) | SQLite `CHECK (host_address = '127.0.0.1')`, port range checks - `migrations/0007_deployment_release.sql:87-89`; internal health checker rejects non-loopback before connecting (`src/adapters/health_check_internal.rs`) | `src/adapters/health_check_internal.rs::rejects_non_loopback_endpoint_before_connecting`, `rejects_ipv6_loopback_endpoint_before_connecting`; store-level proof in `src/adapters/stores/runtime_store.rs::loopback_check_rejects_foreign_addresses_and_hydration_refuses_them` (CHECK rejects other addresses; hydration refuses them too) | Keep |
| INV-RUN-002 | Container port, host port, health path, and expected status are validated value objects (nonzero ports, absolute whitespace-free path starting `/`, status 100–599). | Value invariant | `ContainerPort::new` `HostPort::new` `HealthCheckPath::new` `HealthCheckStatus::new` - `src/domain/runtime.rs:210-306` | Same (domain) | Manifest conversion (`src/adapters/manifest.rs`); SQLite CHECKs (`migrations/0001_application_catalog.sql:37,47`) | `tests/manifest.rs` boundary tests; `tests/domain_values.rs` persisted-value revalidation | Keep |
| INV-RUN-003 | Logical runtime states are closed (`Starting/Running/Stopped/Failed`); unknown Podman states are preserved as typed `Unknown { status }`, absence is explicit `Missing`. | External-boundary invariant | Closed enum - `src/domain/runtime.rs:146-152`; observed enum - `40-50`; mapping - `src/adapters/local_runtime.rs` | Same split (domain owns sets, adapter classifies) | Store hydration errors on unknown logical states, maps observed states to `Unknown` (`runtime_store.rs:368-421,470-483`) | `src/adapters/local_runtime.rs::maps_podman_states_to_explicit_runtime_states`; `src/adapters/stores/runtime_store.rs::loads_a_typed_runtime_state_and_rejects_invalid_persisted_text` | Keep |
| INV-RUN-004 | Retirement is explicit evidence (`removed_at`); a runtime without retirement is logically active, and retired/active cannot contradict removal timestamps. | Entity invariant | `RuntimeRetirement` - `src/domain/runtime.rs:165-169`; store hydration consistency - `runtime_store.rs:368-421` | Same split | None additional | Partially via `loads_a_typed_runtime_state_and_rejects_invalid_persisted_text`; explicit store tests `tests/deployment_register_runtime.rs::rejects_persisted_retirement_without_a_removed_timestamp`, `rejects_an_active_runtime_row_that_carries_a_removed_timestamp`; the tombstone writers (`mark_runtime_removed`, `mark_starting_runtime_missing`) persist exactly the hydratable encoding (`state = 'removed'` plus `removed_at`) | Keep |
| INV-RUN-005 | Stable external identity is `pneuma-<application>-<deployment-id>` shared by container name, Quadlet file `<base>.container`, and systemd service `<base>.service`. | Cross-object rule | `stable_runtime_name` - `src/domain/runtime.rs:381-385` | Same (domain derives; adapters materialize) | Reconciliation matches containers by this deterministic name | `tests/cli.rs::deploy_writes_boot_enabled_quadlet_unit`; reconcile rematerialization tests in `tests/cli.rs`; in-file `src/domain/runtime.rs::stable_names_couple_the_application_and_deployment_identities` | Keep |
| INV-EXP-001 | Exposure intent and materialization are distinct: `desired_visibility` is operator intent; `materialization_state` records the confirmed Caddy result; changing intent alone activates nothing. | Entity invariant | `Exposure` separating intent/materialization - `src/domain/exposure.rs:272-300` | Same (domain) | Guarded store transitions pin both fields (`exposure_store.rs:222-319`) | `tests/cli.rs` visibility flows; `tests/reconciliation.rs` | Keep |
| INV-EXP-002 | Materialization evidence combinations are legal only as encoded: Active requires a ConfirmedRoute; Failed/Diverged require a diagnostic; route triple (runtime id, config version, timestamp) is all-or-none. | Entity invariant | `ExposureMaterialization::hydrate` - `src/domain/exposure.rs:235-270`; `ConfirmedRoute::new` - `118-134` | Same (domain) | Store load enforces presence triples before calling hydrate (`exposure_store.rs:96-219`) | `tests/application_specification.rs::rejects_invalid_persisted_exposure_values`; in-file `src/domain/exposure.rs::confirmed_routes_require_a_trimmed_materialization_timestamp`; `migrations` CHECK/FK test `exposure_materialization_columns_enforce_state_and_runtime_identity` | Keep |
| INV-EXP-003 | Configuration version is the canonical fragment content (domain + loopback endpoint), never a Release or Deployment ID. | Cross-object rule | Fragment builder + `ExposureConfigurationVersion` - `src/adapters/caddy_exposure.rs`, `src/domain/exposure.rs:92-108` | Same split (adapter computes content, domain types it) | Reconciliation compares fragment contents to detect divergence | `tests/caddy_exposure.rs`; reconcile repair tests in `tests/cli.rs` | Keep |
| INV-REC-001 | Reconciliation loads persisted facts in a short transaction and closes it before observing Podman, Quadlet, and Caddy; decisions consume typed observation inputs. The input type groups facts by authority: `DesiredState` (intent), `PersistedState` (bookkeeping), and observed facts stay in `ReconciliationObservation`. The drift answer is a pure domain function: `decide(input, observation, expectations) -> ReconciliationDecision` classifies InSync, runtime identity repair, rematerialization, internal-route removal, public-route materialization, public-exposure failure records, or manual intervention with no store, filesystem, Podman, systemd, Caddy, clock, or randomness access; the use case only acquires ownership, observes, decides, then executes the decided variant (interrupted-deployment recovery remains use-case compensation orchestration). | Workflow invariant | Input loading - `src/use_cases/reconciliation/load.rs`; pipeline orchestration - `src/use_cases/reconciliation/mod.rs`; input types - `src/domain/reconciliation.rs:16-45`, observation types `84-93`; decision function - `decide` in `src/domain/reconciliation.rs` | Same split (use case orders, domain decides and shapes facts; adapters render canonical expectations) | Per-application kernel lock defers concurrent reconcile | `tests/cli.rs::reconcile_defers_before_external_observation`; `tests/reconciliation.rs::loads_active_snapshot_without_writing_sqlite`; in-file decision matrix in `src/domain/reconciliation.rs` | Public route confirmation compares against boundary-rendered canonical fragments; the unreachable `domain_missing` failure classification was removed because validated `ExposureIntent::Public` guarantees a domain |
| INV-REC-002 | After lock release, an interrupted non-terminal Deployment is recorded failed without external effects; candidate cleanup requires provable persisted+external identity. | Workflow invariant | `recover_interrupted_deployment` - `src/use_cases/reconciliation/recover.rs` | Same (use case) | CAS-guarded writes; identity match before cleanup | `tests/reconciliation.rs::reconcile_marks_an_interrupted_pending_deployment_failed_without_external_effects`, `reconcile_cleans_a_verified_candidate_only_after_unit_identity_is_proven`, `reconcile_reports_manual_intervention_when_a_candidate_identity_cannot_be_proven`, `reconcile_reports_manual_intervention_when_an_interrupted_candidate_has_no_persisted_runtime`, `reconcile_marks_an_interrupted_activation_route_diverged_when_prior_route_is_unproven`, `reconcile_preserves_a_proven_prior_route_when_an_activation_was_interrupted`; repair/rematerialize family in `tests/cli.rs` | Keep |
| INV-REC-003 | `Missing` is an observation, not a tombstone; `removed_at` is reserved for candidate cleanup, retirement, and intentional removal. Reconcile never creates a new Deployment/RuntimeInstance because a container is missing. | Cross-object rule | Decision owner - pure domain policy (`classify_runtime_rematerialization` refuses to invent resources) - `src/domain/reconciliation.rs`; design contract - the v0.4 reconciliation design (Git history, commit `6a37693` predecessor series; Invariants 1–2) | Same (domain decides, use case executes) | Store retirement semantics (`INV-RUN-004`) | In-file decision matrix in `src/domain/reconciliation.rs` (rematerialization only for Missing containers of the confirmed identity); reconcile repair tests in `tests/cli.rs` | Keep |
| INV-REC-004 | Every reconciliation recovery/repair action follows the documented contract ("Reconciliation Recovery And Compensation Contract" below): persistence reservation before external effect, explicit confirmation after observation, targeted conditional persistence where an exact precondition matters, defined partial-failure compensation that is never silent success, and re-runnable idempotent effects. | Workflow invariant | `src/use_cases/reconciliation/recover.rs`, `execute.rs`; store conditional primitives in `runtime_store.rs`/`exposure_store.rs` | Same split (use cases order, adapters persist/execute) | Per-Application kernel lock (INV-WF-007) | Tests listed per action in the contract section below | Keep |
| INV-REC-005 | Unknown external state is classified explicitly and never silently adopted as a known-safe value: Podman statuses outside the known vocabulary become `ObservedRuntimeState::Unknown { status }` with raw text preserved (persisted round-trip tolerates unknown text as `Unknown`); unknown recorded/named container states fall through to manual intervention or `UnhandledDrift`; generated-unit active states outside systemd's documented not-running family (`inactive`, `failed`) block automatic rematerialization — so an external system evolving beyond this Pneuma version can never produce a silent stopped/running/succeeded transition. | External-boundary invariant | Raw-value mapping - `observed_state` in `src/adapters/local_runtime.rs`; persisted conversions - `src/adapters/stores/persistence.rs`; conservative decision - `known_not_running_unit_state` + `decide` in `src/domain/reconciliation.rs` | Same split (adapters classify raw values, the pure domain decision refuses to guess) | Execution confirms only exact `Running` observations before CAS persistence (`src/use_cases/reconciliation/execute.rs`); health statuses convert through `HealthCheckStatus::new` (`INV-RUN-002`) | `src/adapters/local_runtime.rs::maps_podman_states_to_explicit_runtime_states`; `src/adapters/stores/persistence.rs::observed_runtime_states_round_trip_with_unknown_tolerated_as_unknown`; in-file matrix `unknown_recorded_runtime_state_requires_manual_intervention_while_running_is_desired`, `unknown_recorded_runtime_state_is_refused_when_stopped_is_desired`, `unknown_or_transient_generated_unit_states_block_rematerialization`, `failed_generated_unit_permits_rematerialization` | Keep |
| INV-WF-001 | Persist intent before external effect; persist confirmed completion after observing the effect (deploy intent, start/stop intent, exposure applying/removing). | Workflow invariant | Use-case sequencing - `src/use_cases/application/runtime.rs`, `src/use_cases/exposure/mod.rs`, `deployment/candidate.rs` | Same (use cases) | Guarded store transitions make out-of-order writes stale | `tests/cli.rs::public_visibility_without_a_domain_is_rejected_before_external_effects`; lifecycle idempotency tests | Keep; ordering assertions remain scenario-level |
| INV-WF-002 | No SQLite transaction remains open during Git, OCI, Podman, systemd, Caddy, or HTTP work. | Workflow invariant | Use-case structure (transactions scoped to store calls only) across `src/use_cases/` (see "Transaction And External Effect Boundaries" below) | Same (use cases) | Writer-lock acquisition inside immediate transactions only (`tests/deployment_create.rs::immediate_transaction_acquires_the_writer_lock_before_reading`) | The CLI E2E fakes (`podman`, `systemctl`, `caddy`, `curl` in `tests/cli.rs`) fail with exit 90 whenever the database rollback journal exists at effect time (`PNEUMA_ASSERT_CLOSED_DATABASE`); the journal guard itself is contract-tested by `fake_external_commands_fail_when_the_database_has_an_open_write_transaction`, and every deploy/lifecycle/visibility/reconcile scenario runs with the guard enabled | Keep |
| INV-WF-003 | Public promotion atomically records succeeded Deployment, active Deployment ID, current RuntimeInstance, and active Exposure in one transaction. | Workflow invariant | Promotion transactions - `src/use_cases/deployment/promotion/` | Same (use cases own transaction boundaries) | Unique indexes catch partial states (`one_current_runtime_per_application`) | `tests/deployment_promote_internal.rs::replaces_the_previous_current_runtime_atomically`, `promotes_healthy_candidate_idempotently` | Keep |
| INV-WF-004 | A failed candidate never replaces the prior active runtime or public route; cleanup removes only resources proven to belong to that candidate; prior-runtime retirement after promotion is best effort. | Workflow invariant | Cleanup - `src/use_cases/deployment/cleanup.rs`; execute-release compensation paths | Same (use cases) | Promotion atomicity means old route persists until success | `tests/deployment_promote_internal.rs::unhealthy_candidate_fails_without_replacing_current_runtime`; `tests/cli.rs::failed_public_health_restores_previous_fragment_and_keeps_public_intent`, `restores_previous_public_route_when_external_health_fails`; `cleanup_does_not_remove_already_promoted_runtime` | Keep |
| INV-WF-005 | Materialization failure compensates by restoring the previous Caddy fragment; incomplete compensation records `diverged` for manual intervention, never silent success. | Workflow invariant | Exposure change/promotion compensation - `src/use_cases/exposure/mod.rs`, `deployment/promotion/public.rs` | Same (use cases) | `ExposureOutcome::{Failed,Diverged}` typed outcomes; CAS confirmation | `tests/cli.rs::lost_public_completion_cas_restores_the_fragment_and_is_not_success` | Keep |
| INV-WF-006 | Start/stop are idempotent; repeating visibility requests matching current desired visibility succeed without touching materialization. | Workflow invariant | `src/use_cases/application/runtime.rs`, `src/use_cases/exposure/mod.rs` | Same (use cases) | Typed intent comparisons before effects | `tests/cli.rs::stop_and_start_are_idempotent...`; visibility repeat tests | Keep |
| INV-WF-007 | One live per-Application kernel lock serializes every mutation of existing Application state: deploy, rollback, lifecycle, status observation persistence, visibility, and reconciliation. It is acquired before the workflow's first state-dependent read and held through effects, confirmation, and compensation; reconciliation defers while it is held. | Workflow invariant | `src/adapters/application_lock.rs` + use-case acquisition | Same split (adapter owns flock mechanics, use case acquires) | Stable lock-file inode; process death releases the lock; targeted conditional writes still surface stale preconditions | `src/adapters/application_lock.rs` same-Application, independent-Application, and process-death tests; deploy contention and reconciliation deferral CLI scenarios | Keep |
| INV-DB-001 | Only one non-terminal Deployment may exist per Application. | Persistence invariant | Partial unique index `one_active_deployment_per_application` - `migrations/0007_deployment_release.sql:73` (originally `0002:43`) | Same | Use case checks before insert; kernel lock serializes attempts | `tests/deployment_create.rs::rejects_a_second_active_deployment`; `src/adapters/stores/deployment_store.rs::partial_unique_index_rejects_a_second_nonterminal_deployment`; `tests/cli.rs::a_second_deploy_is_rejected_while_the_first_is_starting` | Keep |
| INV-DB-002 | Releases, Deployments referencing them, and Runtimes referencing those Deployments all belong to the same Application; mismatches are rejected by triggers on insert and update. | Persistence invariant | Triggers - `migrations/0009_deployment_release_application.sql`, `migrations/0010_runtime_deployment_application.sql`; FKs elsewhere | Same | Domain constructors carry application IDs through all stores | `tests/deployment_create.rs::database_rejects_a_release_from_another_application`; `tests/deployment_register_runtime.rs::database_rejects_a_runtime_identity_from_another_application` | Keep |
| INV-DB-003 | A live loopback endpoint is unique while a runtime is not removed; each candidate reserves its port before registration and reservations are consumed/released exactly once. | Persistence invariant | Unique index `active_runtime_endpoint` - `migrations/0007_deployment_release.sql:132`; reservation PK on `port` - `migrations/0012_runtime_port_reservations.sql:2`; allocator immediate transaction - `src/adapters/port_allocator.rs:49-114` | Same | Allocator checks live runtimes UNION pending reservations atomically | `tests/deployment_register_runtime.rs::database_rejects_a_duplicate_active_endpoint`, `identical_retry_is_idempotent_but_conflicting_reuse_is_rejected`; in-file allocator exclusivity/exhaustion/duplicate-PK tests (see Persistence Concurrency Formalization record 4) | Keep |
| INV-DB-004 | A persistence write is conditional only when correctness depends on an exact prior mutable state. A zero-row conditional write is stale/concurrent/not-converged, never success; ordinary lock-protected observations, inserts, structural constraints, and idempotent deletes do not gain artificial CAS fields. | Persistence invariant | Store conditional primitives and their use-case callers | Same (stores own conditional mechanics; use cases select the needed precondition) | Per-Application lock serializes live workflows; SQLite constraints enforce structural state | `src/adapters/stores/deployment_store.rs::compare_and_set_reports_updated_then_stale`; lost-completion CLI scenarios | Keep |
| INV-DB-006 | Corrupt, invalid, or obsolete persisted values are conversion errors, never silently mapped to invented defaults or compatibility variants. Entity identifiers validate the current store-generated lowercase hexadecimal format at generation and hydration; source revisions must be full commit SHAs; sources must be remote Git URLs. No legacy tolerances remain. | Persistence invariant | All store row mappers (`application_store.rs`, `deployment_store.rs`, `runtime_store.rs`, `exposure_store.rs`, `system_store.rs`, `release_store.rs`) via the shared `entity_id` hydration helper and validated domain constructors | Same (adapters own encoding; domain validates) | Domain constructors reject at hydration time | `tests/domain_values.rs`; `tests/application_list.rs::rejects_legacy_applications_without_a_system`; `src/adapters/stores/deployment_store.rs::source_revision_hydration_requires_a_full_commit_sha`; `src/domain/identity.rs` format tests | Keep |
| INV-DB-007 | Migrations are immutable, forward-only, recorded in `schema_migrations`, applied on connection open with foreign keys enabled; upgrades are tested fresh and from the immediately preceding schema. | Persistence invariant | Migration runner - `src/adapters/database.rs:286-345` | Same (adapter) | Backup/restore commands for downgrade recovery | `open_configures_and_migrates_database`, `migration_is_idempotent`, per-step upgrade chain tests incl. backfill assertions in `src/adapters/database.rs` | Keep |
| INV-SRC-001 | Git revisions are peeled to immutable commits via `<rev>^{commit}` with `--verify --end-of-options`; failures are classified (repository/auth/branch/commit), never collapsed. | External-boundary invariant | `src/adapters/git_source.rs:221-299` | Same (adapter converts to safe domain types: `CommitSha`) | Domain `CommitSha` re-validates 40-char lowercase hex (`src/domain/git.rs:164`) | `tests/git_source.rs::commit_sha_accepts_full_hex_sha`, `rejects_invalid_identifier`; resolution tests | Keep |
| INV-SRC-002 | Checkouts are isolated detached clones (`--no-hardlinks`, destination must not pre-exist, failed checkouts removed, reuse requires clean tree at same HEAD). | External-boundary invariant | `src/adapters/git_source.rs:310-476` | Same (adapter) | Import removes checkout after persistence | `tests/git_source.rs` clone/reuse tests; `tests/cli.rs` import cleanup tests | Keep |
| INV-SRC-003 | Pulled images are digest-verified against the declared artifact; mismatch is a hard error; only canonical lowercase sha256 digests accepted. | External-boundary invariant | `src/adapters/oci_image.rs:142-154,281-297` | Same (adapter verifies; domain types identity) | `OciArtifact` validation before pull (`INV-REL-001`) | `src/adapters/oci_image.rs` tests; `tests/deployment_from_oci.rs::verified_digest_deployment...`; `tests/oci_image.rs` (3 ignored - SKIP: need rootless Podman host) | Run ignored registry tests on a configured rootless Podman host |
| INV-SRC-004 | All systemd control is `--user`; unit absence (exit 4) maps to Missing; unit removal is idempotent; boot-start comes from `WantedBy=default.target`, never `systemctl enable`. | External-boundary invariant | `src/adapters/systemd_quadlet.rs:128,162-262` | Same (adapter) | Quadlet content asserted in deploy scenarios | `tests/cli.rs::deploy_writes_boot_enabled_quadlet_unit`; lifecycle/remove-container cycles; `src/adapters/systemd_quadlet.rs::control_invokes_user_systemctl_with_the_expected_service`, `generated_unit_observation_maps_absence_and_inactive_states` | Keep |
| INV-EXT-001 | Internal health checks connect only to loopback endpoints, use bounded retries (5 attempts × 2 s timeout × 500 ms interval), read a capped status line, and classify timeout vs unreachable. | External-boundary invariant | `src/adapters/health_check_internal.rs:10-12,76-194` | Same (adapter; bounds fixed in production) | Domain endpoint loopback validation upstream (`INV-RUN-001`) | 8 in-file unit tests incl. `rejects_non_loopback_endpoint_before_connecting` | Keep |
| INV-EXT-002 | External health pins the configured domain to loopback via `curl --resolve <domain>:443:127.0.0.1` with proxy bypass, bounded attempt window, and long bounded ACME retry; status must equal expected. | External-boundary invariant | `src/adapters/health_check_external.rs:61-117` | Same (adapter) | Health spec validated at import (`INV-RUN-002`) | `tests/cli.rs` asserts `--resolve` usage (~line 536) and public-health failure paths | Isolated unit tests for the external checker's timeout/retry semantics (known gap) |
| INV-EXT-003 | Managed Caddy fragments live at `<application-id>.caddy`, are imported by the main Caddyfile, and untrusted fragment coordinates (path traversal/unexpected names) are rejected before external work. | External-boundary invariant | `src/adapters/caddy_exposure.rs` | Same (adapter) | Exposure store guards route identity to application | `tests/caddy_exposure.rs::rejects_untrusted_fragment_coordinates_before_external_work` (+12 file tests) | Keep |
| INV-EXT-004 | Port allocation respects the configured `PNEUMA_RUNTIME_PORT_RANGE`; malformed ranges (zero, inverted, non-numeric bounds) are rejected. | External-boundary invariant | `src/adapters/port_allocator.rs:10-11,116-130` | Same (adapter) | Reservation exclusivity in SQLite (`INV-DB-003`) | In-file allocator tests: `rejects_malformed_zero_and_inverted_ranges` (range grammar incl. zero/inverted bounds), exhaustion and exclusivity tests against the default range | Keep |
| INV-EXT-005 | Every external operation carries an explicit idempotency/retry classification (see "External Operation Idempotency And Retry Classification" below): effects are idempotent, convergent, observation-gated, or cleanup-coupled; no path relies on an interrupted operation having completed atomically. | External-boundary invariant | Adapters own per-command semantics (`systemd_quadlet.rs`, `local_runtime.rs`, `caddy_exposure.rs`, `oci_image.rs`, `git_source.rs`, `port_allocator.rs`); use cases gate controls behind fresh observation and own compensation ordering | Same split (adapters classify commands, use cases order retries/cleanup) | Per-Application lock serializes mutations (INV-WF-007); targeted conditional writes make lost preconditions explicit (INV-DB-004) | Quadlet retry tests in `src/adapters/systemd_quadlet.rs`; `tests/caddy_exposure.rs::removes_an_absent_fragment_without_failing_so_removal_is_safe_to_retry`; `tests/git_source.rs::clones_a_repository_by_url_and_cleans_up_the_checkout` (repeat cleanup tolerated) | Keep |
| INV-CI-001 | The restricted SSH dispatcher permits only `version` and `deploy <application> <branch-or-tag>`; both arguments are validated with domain rules; injection attempts are rejected. | Entity invariant | `parse_ci_command` - `src/use_cases/ci/mod.rs` (rules in library, correct owner); `src/cli/ci.rs` only plumbs `SSH_ORIGINAL_COMMAND` | Same | Dispatcher key reaches only this restricted path (security model) | 13 in-file unit tests incl. `parse_injection_attempts_rejected`, `valid/invalid_application_names` | Keep |

## Reconciliation Recovery And Compensation Contract

Every repair and recovery action has
an explicit rule and test (INV-REC-004). Facts shared by all paths:

- **Coordination** - the per-Application kernel lock serializes all work
  (INV-WF-007). Conditional persistence is used only where a path depends on an
  exact precondition; its stale outcome is explicit (INV-DB-004).
- **Retry** - every path is safe to re-run: decisions are re-derived from fresh
  observation on each reconcile, reservations are consumed once, and effects
  are either naturally idempotent or guarded so a repeat converges.
- **Transactions** - never held across external effects (INV-WF-002).

Per action (rule owner first, then test):

1. **Interrupted Pending deployment** (`recover.rs`, Pending arm;
   `tests/reconciliation.rs::reconcile_marks_an_interrupted_pending_deployment_failed_without_external_effects`).
   Precondition: lock released with a non-terminal Pending Deployment. Effect:
   none external. Confirmation: not applicable. Persistence: CAS
   `fail_deployment` records Failed with code `operation_interrupted`. Partial
   failure: stale CAS surfaces as an error, next reconcile retries. Retry/
   idempotency: terminal after one success; later runs skip via the
   non-terminal gate. Compensation: none needed — nothing was materialized.

2. **Interrupted candidate (Starting/Verifying)** (`recover.rs`;
   `tests/reconciliation.rs::reconcile_cleans_a_verified_candidate_only_after_unit_identity_is_proven`,
   `reconcile_reports_manual_intervention_when_a_candidate_identity_cannot_be_proven`,
   `reconcile_reports_manual_intervention_when_an_interrupted_candidate_has_no_persisted_runtime`).
   Precondition: non-terminal candidate plus its persisted runtime row; without
   that row cleanup ownership cannot be proven ⇒ ManualIntervention. Effect:
   stop/remove proven unit, remove proven container, mark runtime missing,
   release port. Confirmation: unit bytes equal the canonical unit AND full
   container identity match (id, name, image reference, application/digest
   labels, endpoint) before any removal; unprovable identity ⇒ ManualIntervention
   with zero cleanup. Persistence: failure recorded first, retirement after the
   external effects (`mark_starting_runtime_missing` CAS). Partial failure:
   cleanup errors abort as NotConverged leaving partial resources for the next
   run (each step individually idempotent). Ownership: use case orchestrates;
   `cleanup_failed_candidate` owns adapter effects. Compensation: deliberately
   none — nothing unproven is ever removed.

3. **Interrupted activation (Activating)** (`recover.rs`;
   `tests/reconciliation.rs::reconcile_marks_an_interrupted_activation_route_diverged_when_prior_route_is_unproven`,
   `reconcile_preserves_a_proven_prior_route_when_an_activation_was_interrupted`).
   Precondition: non-terminal Activating Deployment. Effect: none external —
   the prior route is never touched. Confirmation: prior canonical route proven
   only when the confirmed route matches the active runtime AND the on-disk
   fragment equals the recorded configuration version. Persistence: deployment
   marked failed; exposure failure recorded from the Applying reservation —
   Failed when the prior route is proven preserved, Diverged otherwise; stale
   exposure ⇒ ManualIntervention. Retry: re-records only while the reservation
   still matches. Compensation: none required because no new effect happened.

4. **Runtime identity repair** (`execute.rs::confirm_runtime_identity`;
   `src/adapters/stores/runtime_store.rs::identity_cas_is_stale_unless_the_recorded_container_id_matches`;
   `tests/cli.rs::reconcile_repairs_a_confirmed_quadlet_container_recreation`).
   Precondition: pure decision proved a recreated container's full identity.
   Effect: none external. Persistence: single CAS swap of
   `external_runtime_id`. Partial failure: stale ⇒ NotConverged, retried by the
   next reconcile. Idempotency: repeating with the same observation converges.

5. **Runtime rematerialization** (`execute.rs::rematerialize_runtime`;
   `tests/cli.rs::reconcile_rematerializes_a_missing_quadlet_and_container`,
   `reconcile_restarts_a_canonical_quadlet_after_its_container_is_removed`,
   `reconcile_reports_manual_intervention_for_a_divergent_recreated_container`).
   Precondition: decision proved container Missing (and optionally divergent
   Quadlet bytes) with a startable generated unit. Effect: canonical unit write
   + daemon-reload only when needed, then systemd start. Confirmation: full
   container identity re-observed and matched, then internal health check.
   Persistence: identity CAS confirm strictly after healthy observation.
   Partial failure: absent/divergent rematerialization ⇒ Failed/ManualIntervention
   without persistence; health failure ⇒ Failed; stale CAS ⇒ NotConverged.
   Idempotency: canonical-byte writes and systemd start are idempotent.
   Compensation: none automatic — remaining drift is re-decided next run.

6. **Internal route removal** (`execute.rs::remove_internal_route`;
   `tests/cli.rs::reconcile_removes_an_internal_caddy_fragment`,
   `lost_removal_completion_cas_restores_the_fragment_and_records_failure_during_reconcile`).
   Precondition: decision RemoveInternalRoute carrying the persisted snapshot
   state. Persistence-first: CAS reservation to Removing before any effect.
   Effect: managed fragment removal + Caddy validate/reload. Confirmation/
   Persistence: atomic completion CAS Removing→NotMaterialized clearing the
   route triple. Partial failure: removal error ⇒ failure record flagged by
   `recovery_failed()`; lost completion CAS ⇒ restore removed fragment, then
   record `exposure_changed` (Diverged if restoration also failed). Idempotency:
   removing an already-absent fragment is decided away before any effect.
   Ownership: use case orders reserve→effect→confirm; adapter owns files/Caddy.
   Compensation: `restore_removed_caddy_fragment`.

7. **Public route materialization** (`execute.rs::materialize_public_route`;
   `tests/cli.rs::reconcile_repairs_a_missing_public_caddy_fragment_with_configured_caddyfile`,
   `reconcile_records_failed_public_exposure_when_external_health_cannot_confirm_it`).
   Precondition: decision MaterializePublicRoute for public intent with an
   active runtime. Persistence-first: CAS reservation to Applying. Effect:
   canonical fragment materialize + Caddy validate/reload, then external health
   check pinned to loopback. Confirmation/persistence: completion CAS
   Applying→Active writing the route triple. Partial failure: materialization
   error ⇒ failure record; health failure ⇒ restore previous fragment + record;
   lost completion CAS ⇒ restore + record. Idempotency: canonical bytes are
   deterministic. Compensation: `restore_materialized_caddy_fragment`; incomplete
   compensation records Diverged, never silent success (INV-WF-005).

8. **Public exposure failure record**
   (`execute.rs::record_public_exposure_failure`; unhealthy/missing-runtime
   scenarios in `tests/cli.rs`). Precondition: pure decision classified
   RuntimeMissing/RuntimeNotHealthy carrying exact persisted codes. Effect:
   none external. Persistence: single CAS diagnostic record valid only while
   the expected reservation is current; stale ⇒ NotConverged surfaced as error.
   Idempotency: bounded by the reservation; a stale record defers to whatever
   changed the state.

## Persistence Concurrency Formalization

Every concurrency-sensitive persistence rule with its logical check, its database 
protection, its conflict behavior, and the race/conflict test that proves it. 
The exit criterion is that no concurrent invariant relies on "the CLI is normally 
serial" — every rule below holds under arbitrary interleaved writers because 
serialization comes from the per-application kernel lock (INV-WF-007), SQLite 
immediate transactions, unique indexes/constraints, and compare-and-set writes, 
never from process discipline.

1. **One non-terminal Deployment per Application** (INV-DB-001).
   Rule: at most one Deployment per Application is Pending/Starting/
   Verifying/Activating at any time.
   Logical check: `create_deployment_in_transaction` loads the blocker
   before inserting (`src/use_cases/deployment/create.rs`), inside an immediate
   transaction while the Application lock is held.
   Database protection: partial unique index
   `one_active_deployment_per_application` over non-terminal statuses
   (`migrations/0007_deployment_release.sql:73`) rejects a concurrent insert
   even if it skips the workflow check.
   Conflict behavior: workflow conflict ⇒ typed
   `CreateDeploymentError::ActiveDeployment`; index violation at the boundary
   ⇒ persistence error carrying the constraint failure — both are explicit,
   neither continues as if the write happened.
   Race/conflict test:
   `tests/deployment_create.rs::rejects_a_second_active_deployment_for_the_application`
   (workflow),
   `src/adapters/stores/deployment_store.rs::partial_unique_index_rejects_a_second_nonterminal_deployment`
   (index defense, plus terminal rows exempt).

2. **Compare-and-set persistence writes** (INV-DB-004).
   Rule: every state-racing UPDATE carries an expected prior value; zero rows
   updated is stale/concurrent state, never success.
   Logical check: CAS primitives return `PersistenceOutcome::{Updated,Stale}`
   (`src/adapters/stores/persistence.rs`; deployment/application/runtime/
   exposure stores) and use cases translate Stale into typed conflicts.
   Database protection: single-statement conditional UPDATEs; SQLite statement
   atomicity makes check-and-write indivisible.
   Conflict behavior: `Stale` mapped to explicit errors
   (`RuntimeChanged`, `ExposureChanged`, transition `Conflict`,
   reconciliation `NotConverged`, `mark_failed` `Stale`) — the caller never
   assumes persistence occurred.
   Race/conflict test: `compare_and_set_reports_updated_then_stale`
   (`deployment_store.rs`),
   `runtime_store.rs::identity_cas_is_stale_unless_the_recorded_container_id_matches`,
   exposure store reservation/completion precondition tests, and CLI-level
   lost-CAS scenarios (`tests/cli.rs::lost_public_completion_cas_restores_the_
   fragment_and_is_not_success`,
   `lost_removal_completion_cas_restores_the_fragment_and_records_failure_...
   during_reconcile`).

3. **Runtime endpoint and port-reservation uniqueness** (INV-DB-003,
   INV-EXT-004).
   Rule: a loopback endpoint belongs to at most one live runtime; a candidate
   holds exactly one reserved port before registration; reservations are
   consumed or released exactly once.
   Logical check: the allocator checks live runtimes UNION pending
   reservations inside one immediate transaction before inserting
   (`src/adapters/port_allocator.rs::reserve_port`).
   Database protection: PK on `runtime_port_reservations.port`
   (`migrations/0012_runtime_port_reservations.sql:2`), unique index
   `active_runtime_endpoint` for registered runtimes
   (`migrations/0007_deployment_release.sql:132`); the immediate transaction
   serializes concurrent allocators on the writer lock.
   Conflict behavior: duplicate reservation or endpoint ⇒ constraint
   violation surfaced as a persistence error; exhausted range ⇒ typed
   `PortAllocationError::Exhausted`.
   Race/conflict test: in-file allocator tests
   (`reserves_distinct_ports_and_reuses_a_released_port`,
   `skips_live_runtime_endpoints_and_reuses_removed_ones`,
   `reports_exhaustion_when_every_configured_port_is_reserved`,
   `duplicate_reservations_are_rejected_by_the_primary_key`,
   `rejects_malformed_zero_and_inverted_ranges`);
   `tests/deployment_register_runtime.rs::database_rejects_a_duplicate_active_endpoint`.

## Transaction And External Effect Boundaries

Per-flow local sagas confirming INV-WF-002 and INV-WF-001: every transaction is
short, contains only store calls, and commits before or after — never across —
external effects. Compensation restores external state only after its
transaction has been dropped or committed.

1. **Remote import** (`src/use_cases/application/remote_import.rs`):
   effect = Git clone into an isolated checkout (no persistence open);
   observation = manifest parse; confirmation = one deferred transaction
   inserting system, application, and every manifest-derived specification;
   recovery = checkout cleanup always attempted after the transaction ends.
2. **Branch → OCI delivery** (`src/use_cases/deployment/deploy.rs`):
   effects = Git branch resolve, registry digest resolve, Podman pull (no
   transaction); confirmation = short `create_release` transaction; recovery =
   failures surface before any deployment record exists.
3. **Release deploy / candidate start** (`deployment/create.rs`,
   `candidate.rs`, `execute.rs`): intent = one immediate transaction creating
   the Pending Deployment while the Application lock is held; effects = port reservation
   (own immediate transaction), unit write, daemon-reload, unit start,
   container resolve and observe (no transaction); confirmation = register
   runtime transaction, reservation consumption, CAS transitions to Starting /
   Verifying; recovery = `fail_deployment` CAS then candidate cleanup
   (systemd/Podman removals outside transactions, persisted marks after).
4. **Internal promotion** (`deployment/promotion/internal.rs`): observation = internal
   health check before any transaction; confirmation = one immediate
   transaction atomically stopping other runtimes, starting the target,
   marking the Deployment Succeeded, and activating it on the Application.
5. **Public activation** (`deployment/activation.rs`, `promotion/public.rs`):
   intent = `begin_public_exposure` CAS reserve; effects = Caddy fragment
   materialization + external health check (no transaction); confirmation =
   single immediate promotion transaction; recovery = restore prior fragment
   and record Failed/Diverged via CAS after dropping the transaction.
6. **Exposure change** (`src/use_cases/exposure/mod.rs`): intent =
   `begin_change` transaction persisting Applying/Removing; effects = container
   observation, Caddy materialization/removal, external health (no
   transaction); confirmation = completion transaction; recovery = drop the
   transaction first, then restore the fragment and record failure via CAS.
7. **Reconciliation** (`reconciliation/mod.rs` pipeline): load transaction
   closed before observation; observation =
   Podman/Quadlet/Caddy; decision = pure domain function; execution per
   decision = reserve CAS → external effect → confirm transaction, restoring
   fragments only after the dropped transaction (INV-REC-004).
8. **Runtime lifecycle** (`src/use_cases/application/runtime.rs`): intent =
   lock-protected desired-state write before control; effect = Podman/systemctl
   start or stop; observation/confirmation = lock-protected observation write. No explicit
   transactions at all.

## External Operation Idempotency And Retry Classification

Recorded by consolidation iteration 29: every external operation audited and
classified as `idempotent`, `safe to retry`, `requires observation before
retry`, `requires cleanup`, or `unsafe without proven identity`. Facts shared by
all rows: every existing-Application mutation holds the per-Application kernel
lock (INV-WF-007), and targeted conditional persistence (INV-DB-004) makes a
lost precondition explicit instead of double-applying.

1. **Quadlet create/update** (`systemd_quadlet.rs::write_unit`) — *idempotent*.
   Canonical deterministic bytes at the stable `<unit>.container` path; a
   divergent on-disk unit is replaced by canonical bytes on the next write.
   Test: in-file `write_unit_rewrites_canonical_bytes_so_updates_are_retry_safe`.
   No cleanup needed.
2. **Quadlet remove** (`remove_unit`) — *idempotent*. A missing unit file is
   success, so candidate cleanup can be retried after partial removal.
   Test: in-file `remove_unit_tolerates_missing_units_and_stays_safe_to_retry`.
3. **systemctl control** (`daemon_reload`, `start`, `stop`) — *safe to retry*.
   daemon-reload is a global idempotent refresh; start/stop of an
   already-converged unit succeed without changing it. Retry after a failure is
   always preceded by fresh observation in the owning use case.
4. **Podman observation** (`observe_container`, `observe_named_container`,
   `resolve_container_id`) — *idempotent* read-only commands; absence is typed
   (`Missing`), never an error, so retries cannot invent or erase resources.
   Contract tests: `src/adapters/local_runtime.rs`
   (`observe_container_reports_missing_without_inspecting`,
   `observe_named_container_parses_identity_labels_and_preserves_absence`,
   `resolve_container_id_validates_podmans_answer`).
5. **Podman container start/stop** (`local_runtime.rs::start_container`,
   `stop_container`; used by `application/runtime.rs`) — *requires observation
   before retry*. A blind repeat can fail benignly (already running/stopped),
   so `transition_application` controls only when fresh observation differs
   from the target (INV-WF-006) and re-observes through a persisted CAS
   afterwards. Quadlet-supervised paths prefer systemctl for convergence.
6. **Podman force-remove during compensation** (`remove_container`) —
   *requires cleanup + proven ownership*. Called only on resources tracked in
   `CandidateResources` or proven by reconciliation identity checks; an already
   removed container is observed as `Missing` on retry instead of being
   blindly targeted again (INV-REC-003).
7. **Caddy fragment materialize** (`materialize_caddy_fragment`) —
   *idempotent effect + requires cleanup on failure*. Canonical bytes depend
   only on domain and endpoint, written atomically (temporary file + rename);
   the previous fragment is captured before overwrite so validate/reload
   failures restore it, and incomplete restoration records Diverged rather
   than silent success (INV-WF-005).
8. **Caddy fragment remove** (`remove_caddy_fragment`) — *idempotent*. An
   absent fragment does not fail removal; validate/reload still run so Caddy
   converges even when a previous attempt already removed the fragment.
   Test: `tests/caddy_exposure.rs::removes_an_absent_fragment_without_
   failing_so_removal_is_safe_to_retry`. The prior fragment stays restorable
   during the call for reload-failure compensation.
9. **Digest-pinned image pull** (`oci_image.rs::pull_image`) — *safe to
   retry*. Pulling the same digest converges; inspect verifies the exact
   digest after every pull, so a retried deploy cannot adopt different bytes
   (INV-SRC-003).
10. **Tag-based digest resolution** (`resolve_image_digest`) — *requires
    observation before persistence*. Tags are mutable; the artifact identity
    comes only from post-pull inspection, so a tag moving between retries
    changes the recorded Release explicitly instead of corrupting one.
11. **Release creation** — *idempotent*: `(application_id, image_digest)`
    uniqueness reuses an existing Release (INV-REL-003).
12. **Temporary checkout clone/create** (`clone_repository`,
    `create_checkout`) — deliberately *not idempotent*: an existing
    destination is rejected instead of replaced, protecting workspace state;
    a failed detached checkout removes its own partial directory, and
    abandoned imports are cleaned afterwards (INV-SRC-002).
13. **Checkout reuse** (`ensure_checkout`) — *idempotent with observation*: a
    clean checkout at the requested commit is reused; dirty or stale leftovers
    from failed deployments are discarded and recreated, making a retried
    branch deployment converge (INV-SRC-002).
14. **Checkout cleanup** (`cleanup_checkout`) — *idempotent*: an already
    removed checkout is tolerated (repeat asserted in
    `tests/git_source.rs::clones_a_repository_by_url_and_cleans_up_the_
    checkout`).
15. **Port reservation/release** (`port_allocator.rs`) — reservation is
    *unsafe without proven identity* by construction: SQLite PK plus immediate
    transaction reject duplicates (INV-DB-003); release/consume are
    *idempotent* DELETEs keyed by deployment, where zero rows are fine.

Exit criterion: reconciliation never depends on an interrupted operation
having "not failed midway". Every reconcile decision derives from fresh
observation of desired, persisted, and external facts; decided effects are
either naturally idempotent (records 1–2, 7–8, 11, 13–14), convergent under
observation gating (3–5, 9–10), reservation-gated before any effect
(INV-REC-004), or refused entirely when ownership cannot be proven (6,
ManualIntervention). Partial failures leave typed observable drift — Failed/
Diverged diagnostics, plain external absence, or NotConverged errors — that
the next reconcile repairs or escalates, never guesses about.
