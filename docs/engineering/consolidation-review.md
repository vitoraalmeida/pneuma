# Consolidation Review — Final Engineering Audit

Status: Historical record (iteration 40, the closing audit of the consolidation
program). Implemented behavior lives in [`architecture/`](../architecture/) and
[`code-guide.md`](../code-guide.md); durable invariants live in
[`architecture/invariants.md`](../architecture/invariants.md).

## Original problem

Before the consolidation, Pneuma's implementation had drifted from its own
declared architecture. The symptoms recorded at baseline
(commit `d88f0f4`):

- domain rules had no explicit owners; business decisions leaked into use
  cases, stores, and adapters;
- invalid states were representable: transitions were enforced by scattered
  `from -> to` checks instead of a single domain authority;
- reconciliation mixed observation, decision, effects, confirmation, and
  persistence inside one 1000+ line workflow file;
- Application and Deployment entities carried no real behavior, while use
  cases duplicated their policy;
- SQLite did not consistently protect concurrency-sensitive invariants, and
  zero-row writes could be mistaken for success;
- `main.rs` was a monolith containing parsing, handlers, output, dependency
  construction, and error translation;
- tests lived at arbitrary levels: deep policy was only exercised through E2E,
  while some store and adapter contracts had no direct tests at all.

## Change

A 40-iteration program (tracked in the private `.pneuma-agent/` context,
baseline `d88f0f4`) moved every rule to its architectural owner:

1. **Domain authority** — validated Value Objects for all boundary-entered
   values (`HealthCheckPath`, `HealthCheckStatus`, `ContainerPort`, `HostPort`,
   `DomainName`, `CommitSha`, `RelativeManifestPath`, `OciRepository`,
   catalog-name VOs); Deployment transitions became domain-owned
   (`DeploymentEvent` × `DeploymentStatus::transition/can_fail/is_terminal`,
   full state×event matrix tested in-file); cross-object rules extracted as
   pure functions (`DeliverySpecification::permits`).
2. **Manifest as external boundary** — all TOML/serde structures moved to
   `src/adapters/manifest.rs` behind one parse→validate→convert step returning
   the domain-safe `ImportSpecification`; the domain holds no TOML detail.
3. **Reconciliation pipeline** — split into load → observe → pure
   `decide()` → execute → recover modules; the decision function is pure and
   infrastructure-free with an exhaustive scenario matrix (25 cells), and
   conservative refusal rules for unknown external states (INV-REC-005).
4. **Use-case organization** — flat files grouped into capability directories
   (`deployment/`, `application/`, `system/`, `reconciliation/`, …) with
   curated surfaces; internal helpers demoted to `pub(crate)`.
5. **Public surface audit** — 655 → 440 `pub` declarations; three genuinely
   dead items deleted.
6. **CLI as interface layer** — Clap tree, dispatch, handlers, output
   rendering, error classification (`CliErrorClass` with stable exit codes
   1–5), and composition root separated; `main.rs` reduced to ~70 lines of
   bootstrap + parse + dispatch + exit-code translation.
7. **Persistence formalization** — shared persisted-value conversion module;
   guarded `activate_deployment` EXISTS write; concurrency rules formalized
   (INV-DB-001…005); transaction closure continuously proven by CLI fakes that
   fail when an open write-transaction journal exists at effect time
   (INV-WF-002); external-operation idempotency classified and tested
   (INV-EXT-005).
8. **Test pyramid correction** — 241 → 361 passing tests plus in-file unit
   matrices; every value/entity invariant has a local test, every store has
   dedicated round-trip/CAS/corruption/legacy tests, every adapter has
   PATH-faked contract tests, rollback happy path and converged-reconcile E2E
   scenarios added. Two real defects surfaced and fixed by these tests:
   unreadable retired-runtime tombstones (iteration 34) and silent adoption of
   unknown systemd unit states (iteration 30).
9. **Documentation synchronization** — architecture, data model, glossary,
   nomenclature, code navigation guide (`docs/code-guide.md`), and design-doc
   supersession notes verified against current code; stale "future
   reconciliation" framing removed.

Final-audit findings fixed here (iteration 40): the user-facing
`UnhandledDrift` reason still claimed "runtime repair and public-route
confirmation are not implemented" although both have been executable since the
pipeline iterations; it now states the truth ("drift has no automatic repair;
manual intervention is required"), pinned by focused unit tests on both error
translations.

## Final state

Verified against the current repository, not historical claims:

- **Coesão** — one responsibility per module: capability-grouped use cases,
  phase-per-file reconciliation pipeline, curated re-export surfaces.
- **Coupling** — the domain imports nothing but `std` (pure types such as
  `std::net`); no rusqlite/Podman/systemd/Caddy/filesystem/process dependency
  exists under `src/domain/`.
- **Local reasoning** — `docs/code-guide.md` traces each user-facing flow
  through CLI entry → use case → domain rule → store → adapter without global
  search.
- **Testabilidade** — policy is testable without external effects: the
  reconciliation decision function runs on in-memory structs only; the
  deployment transition matrix is a pure table test.
- **Explicitness** — typed errors throughout; exit classes are stable and
  unit-tested; illegal transitions are named errors
  (`InvalidDeploymentTransition`); CAS outcomes are explicit
  `Updated/Stale`, never silent success.
- **Consistência** — glossary, code, and docs share the active vocabulary
  (`active`, not `current`; `candidate`, `exposure`, `operation` as defined);
  the last known stale user-facing message was corrected in this audit.
- **Failure behavior** — per-flow local sagas (intent before effect,
  observation-gated confirmation, compensation restores prior canonical state);
  recovery/compensation contract documented (INV-REC-004) and test-proven for
  interrupted deployments, abandoned candidates, lost completion CAS, and
  divergent Caddy fragments.
- **Concurrency** — kernel flock ownership (INV-WF-007), immediate short
  transactions, unique partial indexes, port-reservation PK, CAS everywhere,
  zero-row updates treated as conflict; conflict behavior directly tested at
  store level and end to end via the journal guard.
- **Evolução** — adding a new reconciliation decision touches exactly three
  predictable places: the decision variant + classification in
  `src/domain/reconciliation.rs` (with a matrix cell), the executor arm in
  `src/use_cases/reconciliation/execute.rs`, and output rendering if it yields
  a new result shape.

## Consciously deferred debt

Recorded so future work can schedule it; none blocks the consolidation exit:

1. **External health checker isolation** — no dedicated timeout/retry tests
   for `check_external_health` beyond CLI-fake coverage (INV-EXT-002 gap in
   `invariants.md`).
2. **Ignored Podman registry tests** — three `tests/oci_image.rs` tests require
   a configured rootless Podman host; they must be run and recorded PASS/SKIP
   with reasons there, never assumed green.
3. **Orphaned `application_build_specs` table** — unreferenced by any source
   since migration 0001; removing it requires a migration decision and was
   deliberately left out of the consolidation scope.
4. **Converged running internal exposure falls to manual intervention** — only
   a confirmed public route reaches `no-op`. This is arguably a product gap,
   preserved visibly (test-pinned E2E) rather than silently papered over;
   changing `decide` is a product decision, not cleanup.
5. **Permanent legacy tolerances** — NULL `system_id` rows listed,
   `SourceRevision::Legacy` text, incomplete failure evidence, legacy
   `spec_version = 1`: documented third-state semantics (INV-DB-006), not debt
   to remove, kept listed here so they are not mistaken for accidents.

## Evidence / tests

Validation gates on the final code (all green):

```text
cargo fmt --check                                            PASS
cargo clippy --all-targets --all-features -- -D warnings     PASS (zero warnings)
cargo test --all-features                                    PASS (361 passed, 0 failed, 3 ignored*)
cargo build --workspace --release                            PASS
```

\* The 3 ignored tests are deferred-debt item 2 (rootless Podman host
required).

Representative proof points, verifiable in-tree:

- Domain purity: `rg -n "rusqlite|sqlite|podman|systemd|caddy|std::process|std::fs" src/domain/`
  returns no production match.
- Decision matrix: 27 test functions in `src/domain/reconciliation.rs`;
  transition matrix in `src/domain/deployment.rs`.
- Transaction closure: `fake_external_commands_fail_when_the_database_has_an_open_write_transaction`
  plus the guard enabled on every deploy/lifecycle/visibility/reconcile CLI
  builder in `tests/cli.rs`.
- Store conflicts: CAS staleness, partial unique index, port-PK rejection, and
  corrupt-value hydration tests in `src/adapters/stores/*.rs`.
- Full inventory of invariant IDs, struct-role classification, idempotency
  classification, and recovery contract:
  [`architecture/invariants.md`](../architecture/invariants.md).
