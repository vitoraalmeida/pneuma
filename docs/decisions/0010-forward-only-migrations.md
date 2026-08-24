# ADR-0010 - Forward-Only Immutable Migrations Applied On Open

**Status:** Accepted (retrospective)

## Context

Pneuma persists intent, identity, and history in SQLite, and hosts run for long
periods across binary upgrades. Schema evolution must not corrupt existing
deployed applications, and the tool cannot assume an operator manually runs
schema steps.

## Decision

Schema changes are sequential numbered SQL migration files
(`migrations/NNNN_description.sql`) embedded in the binary. Opening a database
connection applies every pending migration inside one transaction each and
records them in `schema_migrations`; foreign keys are enabled on every open.

Rules:

- historical migrations are immutable; a change is a new numbered file;
- upgrades are tested both on a fresh database and from the immediately
  preceding schema;
- downgrades are unsupported; recovery from a bad upgrade is restoring the
  pre-update backup created by `update-pneuma.sh`/`pneuma database backup`.

## Alternatives Considered

- **Editable migrations:** rejected because already-applied files must produce
  identical schemas everywhere; editing them desynchronizes hosts.
- **Downgrade migrations:** rejected because data written by newer logic
  generally cannot be reversed safely; restore-from-backup is honest.
- **Separate migrate command required before use:** rejected because forgetting
  it would pair new binaries with old schemas; applying on open makes that
  state unreachable.

## Consequences

Upgrading is idempotent and automatic, and every host converges to the same
schema for its binary version. The costs are that schema mistakes require a new
forward migration (or backup restore), and migration files accumulate in the
binary.

## References

- [`../architecture/invariants.md`](../architecture/invariants.md) (INV-DB-007)
- [`../architecture/data-model.md`](../architecture/data-model.md)
