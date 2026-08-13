# Design — application_import Persistence Extraction

**Status:** historical design; implemented during v0.2 consolidation.

**Base:** `589970a` (`refactor(runtime): materialize image digest as runtime identity`)

**Approved design:** retired post-v0.2/pre-v0.3 consolidation design (Step 7).

## Objective

Extract all inline SQL from `application_import.rs` into `application_store.rs`, preserving create-only/idempotent behavior and correcting the returned real state when reimporting already deployed Applications.

## Scope

- Move ID generation, system/application creation, and spec inserts to the store.
- Add `load_application_for_import(&Transaction, name)` with `LEFT JOIN application_sources` and `active_deployment_id`.
- Type store APIs to receive `DeliveryType` and `Visibility`, not strings.
- Correct reimport to return the real state of the existing Application.
- Add coverage for atomicity, OCI without source, and deployed reimport.

## Non-goals

- Do not change the import contract (Git URLs only).
- Do not change the schema or migrations.
- Do not implement updates for divergent specs on reimport.
- Do not change other use cases in this checkpoint.

## Use Case Changes

In `src/use_cases/application_import.rs`:

- Retain `load_manifest_at`, selection of `DEFAULT_MANIFEST_PATH`, and `--system` precedence.
- Retain `SystemRequired`.
- Retain `connection.transaction()` and `commit()` in the use case.
- Replace every `query_row` and `execute` with calls to `application_store`.
- Remove `persist_specification`.
- Convert `ApplicationStoreError` into `ImportError::Persistence`, preserving the underlying `rusqlite::Error`.
- Continue classifying `repository_url` with `is_remote_repository`; this is import context, not an SQL decision.

Flow:

1. Load and validate `pneuma.toml`.
2. Resolve the system name:
   - `--system` takes precedence over `[system].name`.
   - The absence of both returns `SystemRequired` before the transaction.
3. Open a single transaction.
4. Load the existing Application by name using `load_application_for_import`.
5. If it already exists:
   - Return the persisted Application without changing specs or creating a system.
6. If it does not exist:
   - Generate a system ID and call `ensure_system`.
   - Load the effective system ID by name.
   - Generate an Application ID and call `insert_application`.
   - Persist delivery.
   - Persist source only when `repository_url` exists.
   - Persist runtime spec.
   - Persist health spec.
   - Persist exposure.
7. Load the Application by name using the store.
8. Commit and return.

## Store Changes

In `src/adapters/stores/application_store.rs`:

- Reuse existing ID-generation, system, Application, and spec-insert primitives.
- Change APIs to receive domain types:
  - `insert_delivery_spec(..., DeliveryType, ...)`
  - `insert_exposure(..., Visibility, ...)`
- Conversion to `"oci"`, `"public"`, and `"internal"` remains in the store through `database_value()`.
- Keep `repository_kind` as a string in this checkpoint.

Add:

```rust
pub fn load_application_for_import(
    transaction: &Transaction<'_>,
    name: &str,
) -> Result<Option<Application>, ApplicationStoreError>
```

Query:

```sql
SELECT
    a.id,
    a.system_id,
    a.name,
    s.repository_url,
    s.default_branch,
    a.active_deployment_id
FROM applications AS a
LEFT JOIN application_sources AS s
    ON s.application_id = a.id
WHERE a.name = ?1
```

- `LEFT JOIN` preserves OCI imports without `application_sources`.
- `active_deployment_id` must be loaded, not artificially filled with `None`.
- Absence is `Ok(None)`, not an invented ID or `QueryReturnedNoRows`.

## Behavior Correction

Today, when reimporting an already deployed Application:

- `application_import` returns `active_deployment_id: None`, although the database has an active deployment.
- `main.rs` always prints `Deployment: Not deployed` after import.

Correction:

- `load_application_for_import` returns the actual `active_deployment_id`.
- The CLI flow must derive its final line from the returned state:
  - without an active deployment: `Deployment: Not deployed`;
  - with an active deployment: state that it is already deployed, without inventing a new state.

## Tests

In `tests/application_import.rs`:

1. **Remote OCI import without duplicate source**
   - Import `oci-only` twice with a remote `repository_url`.
   - Confirm one Application, one delivery spec, and a single `application_sources`.
   - Confirm the second import creates no additional specs (no duplicate row).

2. **Deployed Application reimport**
   - Import a fixture.
   - Create/persist an active deployment and assign it to `applications.active_deployment_id`.
   - Reimport the same manifest.
   - Confirm the returned `Application` retains `Some(deployment_id)`.
   - Confirm the original source/specs remain unchanged.

3. **Failure in the middle of the aggregate is atomic**
   - Create a temporary trigger that interrupts `INSERT` into `application_runtime_specs`.
   - Import a fixture with a remote URL after delivery/source have already been attempted.
   - Expect `ImportError::Persistence`.
   - Confirm zero records in:
     - `systems`
     - `applications`
     - `application_delivery_specs`
     - `application_sources`
     - `application_runtime_specs`
     - `health_check_specs`
     - `exposures`

4. **Divergent reimport is create-only**
   - Reimport with a divergent system, URL, or manifest.
   - Confirm the existing Application and its specs do not change.
   - Confirm the return represents the persisted Application, not the new arguments.

In `tests/cli.rs`:

5. **CLI reports real state on reimport**
   - Import and deploy an Application.
   - Run `pneuma app import <file-url>` again.
   - Confirm output corresponding to an already deployed Application instead of `Deployment: Not deployed`.

## Implementation Order

1. Add `load_application_for_import`.
2. Type delivery and visibility in the store.
3. Refactor `application_import` to use only store APIs.
4. Correct the deployed-reimport return/output.
5. Add atomicity, OCI-without-source, and deployed-reimport tests.
6. Confirm that no `query_row`, `execute`, `prepare`, or `params!` remains in `application_import.rs`.
7. Run:
   - `cargo test --test application_import`
   - `cargo test --test cli`
    - four complete gates.
8. Update the checkpoint in the tracker and create the commit:

`refactor(store): move application import persistence to application store`

## Acceptance Criteria

- `application_import.rs` contains no inline SQL.
- The store receives `DeliveryType` and `Visibility`, not strings.
- `load_application_for_import` returns the actual `active_deployment_id`.
- Reimport does not change divergent specs.
- Reimport of a deployed Application returns real state.
- Failure in the middle of the aggregate is atomic.
- The CLI reports real state on reimport.
- Four gates are green.
