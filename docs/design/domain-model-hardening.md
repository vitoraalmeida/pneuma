# Design - Domain Model Hardening

**Status:** historical record. Approved for v0.4 before reconciliation and now
fully implemented; implemented behavior lives in
[`../architecture/`](../architecture/). Execution history is in Git.

## Objective

Strengthen Pneuma's domain model before reconciliation relies on it. The model
must distinguish logical identity, persisted intent, and external observation;
prevent invalid new values from being constructed; and reject inconsistent
persisted combinations with context rather than silently normalizing them.

## Fixed Decisions

- Logical IDs for Systems, Applications, Releases, Deployments, and
  RuntimeInstances are distinct opaque domain values. They retain SQLite's text
  representation and do not impose a new historical format.
- External Podman container IDs are distinct from logical RuntimeInstance IDs.
- The RuntimeInstance endpoint is persisted expected identity. Observation never
  replaces its address or port. A different observed endpoint is drift.
- `Missing` remains an external observation. Stopped intent may be satisfied by
  a missing container but does not rewrite that observation to `Stopped`.
- A runtime retirement is explicit evidence. `removed_at` is never inferred from
  a missing observation. Historical `state = 'removed'` rows remain readable
  through a compatibility mapper.
- Container and route observations use state-bearing types, not independent
  optional values or booleans that collapse ambiguity.
- Exposure intent, confirmed route evidence, and materialization diagnostics are
  cohesive values. Public intent requires a valid domain, and an active route
  requires confirmed runtime, configuration version, and materialization time.
- Deployment history carries lifecycle timestamps and complete failure evidence.
  Historical rows lacking evidence remain explicitly incomplete; no invented
  values are introduced.
- Application specifications validate names, OCI repositories, source
  revisions, manifest paths, ports, health paths and statuses, and domains at
  their input boundaries.
- Stores own SQLite spelling, row mapping, persisted-value conversion, and
  compare-and-set result handling. Domain types do not expose database
  conversions.
- The current SQLite schema remains unchanged unless a checkpoint demonstrates
  that a migration is necessary. New validation must preserve readable legacy
  values or report them as contextual store errors.

## Checkpoint Order

1. Introduce typed logical and external identities across domain, stores, use
   cases, CLI errors, and fixtures without changing persisted text.
2. Validate application specification and OCI values, including cohesive source
   representation and shared OCI repository identity.
3. Separate expected runtime identity from observation, preserve `Missing`, and
   represent runtime retirement explicitly.
4. Make exposure intent, materialization evidence, and diagnostics valid by
   construction while preserving compensation-relevant evidence.
5. Add deployment lifecycle evidence and replace scalar and tuple outputs with
   cohesive values.
6. Move persistence conversions into stores and require explicit stale outcomes
   from compare-and-set writes.
7. Add reconciliation-specific snapshots, observations, classifications, and
   outcomes only after the hardened source model is complete.

## Non-goals

- No new SQLite schema or automatic data repair without a checkpoint-specific
  decision and migration coverage.
- No reconciliation command, runtime repair, Caddy repair, or external effect
  in the hardening checkpoints.
- No user-interface implementation or presentation state in domain entities.
- No generic ID framework, traits, async runtime, or new dependency for these
  value objects.
