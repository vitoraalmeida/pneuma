# Pneuma Durable Guarantees

**Status:** living document — the compact inventory of the architecture's
durable guarantees.

A durable guarantee is a property that must hold at every point in time —
across requests, crashes, restarts, concurrent operations, and external
failure. Every guarantee names exactly one owner layer. When changing code,
check the guarantees you touch here first; when adding code, record new rules
here before they become implicit.

Categories:

- **Value** — invalid values are unrepresentable: a validated domain type is
  constructed once at the boundary.
- **Entity** — legality depends on the entity's state together with the
  requested operation.
- **Persistence** — SQLite constraints, immediate transactions, and targeted
  conditional writes protect stored facts against races and corruption.
- **Workflow** — ordering of persistence and external effects within one
  operation.
- **Boundary** — untrusted or open-ended external input is classified once at
  the edge into safe domain types.
- **Reconciliation** — drift decisions and recovery actions over typed facts.

| ID | Guarantee | Category | Owner | Proven by |
|---|---|---|---|---|
| INV-SYS-001 | System names satisfy the shared catalog name rule (1–63 chars, lowercase ASCII letters/digits/hyphens, alphanumeric first and last). | Value | `SystemName::new` and the shared predicate in `src/domain/identity.rs` | In-file boundary tests; `tests/manifest.rs` |
| INV-APP-001 | Application names satisfy the same catalog name rule as Systems. | Value | `ApplicationName::new` (`src/domain/application.rs`) | `tests/manifest.rs`; CI parse tests in `src/use_cases/ci/mod.rs` |
| INV-APP-002 | Application persisted state has exactly three mutation paths: identity (`system_id`) written once at import; runtime intent under the Application lock or through activation; activation writing `active_deployment_id` with running intent only for a succeeded Deployment of that Application. No code mutates a hydrated `Application`. | Persistence | Insert, intent, and guarded activation primitives in `src/adapters/stores/application_store.rs`; eligibility decided by `DeploymentStatus::transition(Activated)` | `tests/application_specification.rs`; `tests/deployment_promote_internal.rs` |
| INV-APP-003 | `Application.system_id` is a required `SystemId`: import resolves or creates exactly one System, and hydration rejects a row without a System. | Persistence | `application_store.rs` insert and hydration; domain validates | `tests/application_list.rs::rejects_legacy_applications_without_a_system`; `tests/application_import.rs` |
| INV-APP-004 | The manifest `schema_version` is an import-boundary check only; no version copy is persisted or carried by the domain. | Boundary | Schema gate in `src/adapters/manifest.rs` | `tests/manifest.rs`; `tests/application_import.rs` |
| INV-MAN-001 | Manifests declare schema version 3 exactly; unknown TOML fields are rejected. | Boundary | `src/adapters/manifest.rs` (deny-unknown-fields DTO plus version gate) | `tests/manifest.rs::rejects_unknown_fields_and_schema_versions` |
| INV-MAN-002 | Delivery is OCI-only; `[delivery].image` is a repository without tags/digests or surrounding whitespace. | Boundary | `OciRepository::new` via the manifest boundary; grammar in `src/domain/release.rs` | `tests/manifest.rs`; `tests/release_create.rs::rejects_invalid_artifact_identity` |
| INV-MAN-003 | Public visibility requires a valid domain; internal may omit it. | Entity | `ExposureIntent::new` (`src/domain/exposure.rs`); baseline `CHECK (desired_visibility = 'internal' OR domain IS NOT NULL)` | `tests/manifest.rs::requires_a_domain_for_public_exposure` |
| INV-REL-001 | A Release artifact identity is always `repository@sha256:<64 lowercase hex>`; mutable tags never become artifacts. | Value | `OciArtifact` (`src/domain/release.rs`); store re-validates persisted references; pull verifies the digest | `src/adapters/oci_image.rs` tests; `tests/release_create.rs`; `tests/deployment_from_oci.rs`; `tests/oci_image.rs` (ignored: needs rootless Podman) |
| INV-REL-002 | An Application permits only the OCI repository recorded from its manifest; foreign repositories are rejected before any pull. | Entity | `DeliverySpecification::permits` applied by the deploy use case before any effect | In-file tests in `src/domain/release.rs`; `tests/deployment_from_oci.rs::repository_not_allowed_before_pull` |
| INV-REL-003 | `(application_id, image_reference)` uniquely identifies a Release; duplicates are reused, never re-created. | Persistence | Baseline unique constraint on `releases`; reuse logic in `src/use_cases/release/mod.rs` | `tests/release_create.rs`; `tests/deployment_create.rs::reuses_a_release_for_a_later_deployment_attempt` |
| INV-DEP-001 | The only legal Deployment transitions are Pending→Starting→Verifying→Activating→Succeeded plus Verifying→Succeeded, with every non-terminal state allowed to fail into Failed; Succeeded/Failed are terminal. | Entity | `DeploymentStatus::transition` over `DeploymentEvent` (`src/domain/deployment.rs`); store CAS primitives persist only approved pairs; baseline status `CHECK` | In-file state×event matrix; `tests/deployment_transition.rs` |
| INV-DEP-002 | A newly recorded failure requires trimmed non-empty code, message, timestamp, and a non-terminal stage. | Entity | `DeploymentFailure::validate_details/new` | `tests/deployment_transition.rs::rejects_incomplete_failure_details_without_changing_state`; in-file tests |
| INV-DEP-003 | A failed Deployment carries complete typed evidence: `DeploymentFailureCode`, the non-terminal stage, a trimmed message, and the terminal timestamp. Hydration rejects incomplete or unknown-code failed rows. | Persistence | Store hydration (`deployment_store.rs`); domain type makes incomplete evidence unrepresentable; baseline evidence `CHECK`s | In-file hydration tests; `tests/deployment_list.rs` |
| INV-DEP-004 | Promotion requires logical state Starting, observed Podman Running, and no retirement; an already-Succeeded deployment returns its promotion idempotently. | Entity | `PromotionTarget` (`src/domain/deployment.rs`); use case executes | `tests/deployment_promote_internal.rs` |
| INV-DEP-005 | Rollback creates a new `rollback` Deployment for the most recent succeeded non-active Release and never edits prior history. | Entity | `RollbackTarget` selection plus `src/use_cases/deployment/rollback.rs` | `tests/deployment_rollback.rs`; `tests/deployment_execute_release.rs::rollback_executes_a_new_deployment_from_historical_provenance` |
| INV-DEP-006 | Semantically relevant Deployment mutations route through domain gates; the remaining writes are deliberately procedural bookkeeping: insert-only creation, terminal timestamp/evidence writes inside CAS primitives, and one-time `started_at` stamping. | Persistence | Store CAS primitives (`advance_status`, `mark_succeeded`, `mark_failed`); domain gates in `src/domain/deployment.rs` | `tests/deployment_transition.rs` (started_at stamped once, preserved after); `compare_and_set_reports_updated_then_stale` |
| INV-RUN-001 | Runtime endpoints are loopback-only (IPv4 127.0.0.1, nonzero port); running observations require a validated endpoint. | Value | `validate_loopback_endpoint` (`src/domain/runtime.rs`); the internal health checker rejects non-loopback before connecting | `src/adapters/health_check_internal.rs` tests; `runtime_store.rs` hydration tests |
| INV-RUN-002 | Container port, host port, health path, and expected status are validated value objects (nonzero ports, absolute whitespace-free path starting `/`, status 100–599). | Value | Constructors in `src/domain/runtime.rs`; manifest conversion; baseline `CHECK`s | `tests/manifest.rs`; `tests/domain_values.rs` |
| INV-RUN-003 | Logical runtime states are closed (`Starting/Running/Stopped/Failed`); unknown Podman states become typed `Unknown { status }`; absence is explicit `Missing`. | Boundary | Closed and observed enums in `src/domain/runtime.rs`; classification in `src/adapters/local_runtime.rs` | `local_runtime.rs` state-mapping tests; `runtime_store.rs` hydration tests |
| INV-RUN-004 | Retirement is explicit evidence (`removed_at` plus a compatible state); a runtime without retirement is logically active; there is no persisted `removed` pseudo-state. | Entity | `RuntimeRetirement` and store hydration consistency; baseline `CHECK (removed_at IS NULL OR state IN ('starting', 'stopped'))` | `tests/deployment_register_runtime.rs` retirement tests |
| INV-RUN-005 | Stable external identity is `pneuma-<application>-<deployment-id>`, shared by container name, Quadlet file, and systemd service. | Entity | `stable_runtime_name` (`src/domain/runtime.rs`); adapters materialize it | `tests/cli.rs::deploy_writes_boot_enabled_quadlet_unit`; reconcile rematerialization tests |
| INV-EXP-001 | Exposure intent (`desired_visibility`) and materialization (`materialization_state`) are distinct; changing intent alone activates nothing. | Entity | `Exposure` (`src/domain/exposure.rs`); guarded store transitions | `tests/cli.rs` visibility flows; `tests/reconciliation.rs` |
| INV-EXP-002 | Materialization evidence combinations are legal only as encoded: Active requires a ConfirmedRoute; Failed/Diverged require a diagnostic; the route triple is all-or-none. | Entity | `ExposureMaterialization::hydrate`; store load enforces triples; baseline `CHECK`s | `tests/application_specification.rs::rejects_invalid_persisted_exposure_values`; in-file tests |
| INV-EXP-003 | The configuration version is the canonical fragment content (domain + loopback endpoint), never a Release or Deployment ID. | Entity | Fragment builder in `src/adapters/caddy_exposure.rs`; `ExposureConfigurationVersion` types it | `tests/caddy_exposure.rs`; reconcile repair tests in `tests/cli.rs` |
| INV-REC-001 | Reconciliation loads persisted facts in a short transaction, closes it before observing Podman/Quadlet/Caddy, and decides with a pure domain function over desired, persisted, and observed facts — no store, filesystem, Podman, systemd, Caddy, clock, or randomness access. | Reconciliation | Pipeline in `src/use_cases/reconciliation/`; `decide` in `src/domain/reconciliation/decision.rs`; per-Application lock defers concurrent reconciles | `tests/cli.rs::reconcile_defers_before_external_observation`; `tests/reconciliation.rs`; in-file decision matrix |
| INV-REC-002 | After lock release, an interrupted non-terminal Deployment is recorded failed without external effects; candidate cleanup requires provable persisted and external identity. | Reconciliation | `src/use_cases/reconciliation/recover.rs` | `tests/reconciliation.rs` interrupted-candidate scenarios |
| INV-REC-003 | `Missing` is an observation, not a tombstone; reconciliation never invents a Release, Deployment, or certainty, and rematerializes only the proven confirmed identity. | Reconciliation | Pure domain policy in `src/domain/reconciliation/decision.rs` | In-file decision matrix; reconcile repair tests in `tests/cli.rs` |
| INV-REC-004 | Every recovery action follows the contract below: persist the reservation before the effect, confirm after observation, use CAS only where an exact precondition matters, and record partial failure instead of silent success. | Reconciliation | `recover.rs`/`execute.rs` order; store conditional primitives; adapters effect | `tests/reconciliation.rs`; `tests/cli.rs` lost-CAS scenarios |
| INV-REC-005 | Unknown external states stay explicitly unknown: Podman statuses outside the known vocabulary become `Unknown`, unknown recorded states require manual intervention, and generated-unit states outside systemd's documented not-running family block automatic rematerialization. | Boundary | Raw-value mapping in `local_runtime.rs`/`persistence.rs`; conservative decision in `domain/reconciliation/decision.rs` | Adapter and persisted round-trip tests; in-file decision matrix |
| INV-WF-001 | Persist intent before the external effect; persist confirmed completion only after observing the effect. | Workflow | Use-case sequencing in `src/use_cases/` (runtime, exposure, deploy) | `tests/cli.rs` lifecycle and visibility scenarios |
| INV-WF-002 | No SQLite transaction remains open during Git, OCI, Podman, systemd, Caddy, or HTTP work. | Workflow | Use-case structure; CLI fake commands fail if the rollback journal exists at effect time (`PNEUMA_ASSERT_CLOSED_DATABASE`) | `tests/cli.rs` (guard enabled in every effect scenario); `fake_external_commands_fail_when_the_database_has_an_open_write_transaction` |
| INV-WF-003 | Public promotion atomically records the succeeded Deployment, active Deployment ID, current RuntimeInstance, and active Exposure in one transaction. | Workflow | Promotion transactions in `src/use_cases/deployment/promotion/` | `tests/deployment_promote_internal.rs` |
| INV-WF-004 | A failed candidate never replaces the prior active runtime or public route; cleanup removes only resources proven to belong to the candidate; prior-runtime retirement after promotion is best effort. | Workflow | Cleanup in `src/use_cases/deployment/cleanup.rs` and execute-release compensation | `tests/deployment_promote_internal.rs::unhealthy_candidate_fails_without_replacing_current_runtime`; `tests/cli.rs` restore scenarios |
| INV-WF-005 | Materialization failure compensates by restoring the previous Caddy fragment; incomplete compensation records `diverged` for manual intervention, never silent success. | Workflow | Exposure change and public-promotion compensation | `tests/cli.rs::lost_public_completion_cas_restores_the_fragment_and_is_not_success` |
| INV-WF-006 | Start/stop are idempotent; repeating visibility requests matching current desired visibility succeed without touching materialization. | Workflow | `src/use_cases/application/runtime.rs`, `src/use_cases/exposure/mod.rs` | `tests/cli.rs` idempotency scenarios |
| INV-WF-007 | One live per-Application kernel lock serializes every mutation of existing Application state — deploy, rollback, lifecycle, status persistence, visibility, and reconciliation — from the first state-dependent read through confirmation and compensation; reconciliation defers while it is held. | Workflow | `src/adapters/application_lock.rs` plus use-case acquisition | In-file same-Application, independent-Application, and process-death tests; `tests/cli.rs` contention scenarios |
| INV-DB-001 | Only one in-progress (non-terminal) Deployment may exist per Application. | Persistence | Partial unique index `one_in_progress_deployment_per_application`; use-case precheck; Application lock | `tests/deployment_create.rs`; `deployment_store.rs` index test; `tests/cli.rs::a_second_deploy_is_rejected_while_the_first_is_starting` |
| INV-DB-002 | Releases, Deployments referencing them, and Runtimes referencing those Deployments all belong to the same Application. | Persistence | Composite foreign keys (`(id, application_id)`) in the baseline schema; domain constructors carry application IDs through all stores | `tests/deployment_create.rs`; `tests/deployment_register_runtime.rs` cross-application tests |
| INV-DB-003 | A live loopback endpoint is unique while its runtime is not removed; each candidate reserves exactly one port before registration; reservations are consumed or released exactly once. | Persistence | Unique index `one_live_runtime_endpoint`; reservation primary key; allocator immediate transaction (`src/adapters/port_allocator.rs`) | Allocator in-file tests; `tests/deployment_register_runtime.rs` |
| INV-DB-004 | A persistence write is conditional only when correctness depends on an exact prior mutable state; a zero-row conditional write is stale/concurrent/not-converged, never success. Ordinary lock-protected writes, inserts, structural constraints, and idempotent deletes gain no artificial CAS fields. | Persistence | Store conditional primitives and their use-case callers; Application lock serializes live workflows | `deployment_store.rs::compare_and_set_reports_updated_then_stale`; runtime identity CAS test; `tests/cli.rs` lost-CAS scenarios |
| INV-DB-006 | Corrupt, invalid, or obsolete persisted values are conversion errors, never invented defaults: entity identifiers validate the current lowercase hexadecimal format at generation and hydration; source revisions are full commit SHAs; sources are remote Git URLs. No legacy tolerances remain. | Persistence | All store row mappers via the shared hydration helpers and validated domain constructors | `tests/domain_values.rs`; `tests/application_list.rs`; store hydration tests |
| INV-DB-007 | The database opens only with the exact current baseline ledger; empty databases initialize atomically; backup/restore accept only the current schema. A database-wide kernel lock (shared for normal commands, exclusive for restore) serializes replacement against every other database user; `version` stays lock-free; restore validates source integrity and schema before any live mutation. | Persistence | `src/adapters/database.rs` (`open`, `backup`, `restore_and_verify`, `DatabaseLock`); shared-lock acquisition and normal connection lifetime in `src/control/mod.rs::ControlExecutor` | Database in-file tests (lock serialization, process-death release, incompatible/corrupt rejection, busy restore); `tests/cli.rs` restore scenarios |
| INV-SRC-001 | Git revisions are peeled to immutable commits via `<rev>^{commit}` with `--verify --end-of-options`; failures are classified (repository/auth/branch/commit), never collapsed. | Boundary | `src/adapters/git_source.rs`; domain `CommitSha` re-validates | `tests/git_source.rs` resolution tests |
| INV-SRC-002 | Checkouts are isolated detached clones (`--no-hardlinks`, destination must not pre-exist, failed checkouts removed); deployments never materialize a checkout. | Boundary | `src/adapters/git_source.rs` (`clone_repository`, `resolve_branch`, `cleanup_checkout`) | `tests/git_source.rs`; `tests/cli.rs` import cleanup tests |
| INV-SRC-003 | Pulled images are digest-verified against the declared artifact; mismatch is a hard error; only canonical lowercase sha256 digests are accepted. | Boundary | `src/adapters/oci_image.rs`; `OciArtifact` validation upstream | `src/adapters/oci_image.rs` tests; `tests/deployment_from_oci.rs`; `tests/oci_image.rs` (ignored: needs rootless Podman) |
| INV-SRC-004 | All systemd control is `--user`; unit absence maps to Missing; unit removal is idempotent; boot-start comes from `WantedBy=default.target`, never `systemctl enable`. | Boundary | `src/adapters/systemd_quadlet.rs` | `tests/cli.rs::deploy_writes_boot_enabled_quadlet_unit`; in-file control tests |
| INV-EXT-001 | Internal health checks connect only to loopback endpoints with bounded retries (5 attempts × 2 s timeout × 500 ms interval), a capped status line, and explicit timeout/unreachable classification. | Boundary | `src/adapters/health_check_internal.rs` | In-file unit tests |
| INV-EXT-002 | External health pins the configured domain to loopback via `curl --resolve <domain>:443:127.0.0.1` with proxy bypass, a bounded attempt window, and a long bounded ACME retry; status must equal expected. | Boundary | `src/adapters/health_check_external.rs` | `tests/cli.rs` (`--resolve` assertion and public-health failure paths) |
| INV-EXT-003 | Managed Caddy fragments live at `<application-id>.caddy`, are imported by the main Caddyfile, and untrusted fragment coordinates are rejected before external work. | Boundary | `src/adapters/caddy_exposure.rs` | `tests/caddy_exposure.rs::rejects_untrusted_fragment_coordinates_before_external_work` |
| INV-EXT-004 | Port allocation respects the configured `PNEUMA_RUNTIME_PORT_RANGE`; malformed ranges (zero, inverted, non-numeric bounds) are rejected. | Boundary | `src/adapters/port_allocator.rs` | In-file allocator tests |
| INV-EXT-005 | Every external operation is idempotent, convergent under observation gating, reservation-gated, or refused without proven identity (classification below). | Boundary | Adapters own per-command semantics; use cases gate controls behind fresh observation and own compensation ordering | Retry tests in `systemd_quadlet.rs`; `tests/caddy_exposure.rs`; `tests/git_source.rs` |
| INV-CI-001 | The restricted SSH dispatcher permits only `version` and `deploy <application> <branch-or-tag>`, validates both arguments with domain rules, and rejects injection attempts. | Entity | `parse_ci_command` in `src/use_cases/ci/mod.rs`; the CLI maps its validated deploy grammar to the same `ControlExecutor` command as interactive deployment | In-file parse tests including injection rejection |

## Recovery Action Contract (INV-REC-004)

Every recovery action — interrupted pending/candidate/activation recovery,
runtime identity repair, runtime rematerialization, internal-route removal,
public-route materialization, and public-exposure failure recording — shares
one contract, owned by `src/use_cases/reconciliation/` with stores persisting
and adapters effecting:

- the persistent reservation (CAS) happens before any external effect, and
  confirmation only after fresh observation;
- external effects are re-runnable: canonical-byte writes, absent-tolerant
  removals, and observation-gated convergence;
- removal happens only after full identity proof, and retirement or missing
  state is recorded only after observed absence;
- partial failure is recorded as Failed, Diverged, or NotConverged — never
  silent success — and the next reconcile re-decides from fresh observation;
  ownership that cannot be proven is escalated to ManualIntervention with zero
  cleanup.

Each action's exact preconditions, confirmations, and compensation are proven
by the corresponding scenarios in `tests/reconciliation.rs` and `tests/cli.rs`.

## External Retry Classification (INV-EXT-005)

Every mutation holds the per-Application lock (INV-WF-007) and targeted CAS
(INV-DB-004) makes lost preconditions explicit instead of double-applying.
Against that background: canonical-byte writes (Quadlet units, Caddy
fragments) and absent-tolerant removals (unit and fragment removal, checkout
cleanup, reservation release) are idempotent; systemctl start/stop, container
start/stop, and tag-based digest resolution require fresh observation before
retry; force-removal during compensation requires proven ownership plus
observed absence; temporary checkout clones deliberately refuse an existing
destination; reservation allocation is constraint-guarded. No path relies on
an interrupted operation having completed atomically.
