# Next Iteration

**Status:** planning reminder, not an execution tracker.

**Target:** v0.6 - Observed State / Host Observation. Queued behind the active
v0.5.3 CLI adapter integrity iteration.

Do not implement this work until an approved design exists and this tracker is
promoted to `current-iteration.md`.

## Objective

Make Pneuma explicitly observe the real state of the host instead of trusting
predominantly the state it recorded itself, establishing the separation between
desired state (what should exist), recorded state (what Pneuma believes it did),
and observed state (what actually exists now).

## Checkpoints

- Workload observation from systemd/Podman: unit existence and state, container
  existence and running state, current PID, image/digest in use, exit status.
- Proxy observation from Caddy: route presence, domain-to-target correctness,
  absence of routes that should be absent, exposure divergence from desired
  state.
- Explicit observed-state model with a small verdict set comparing desired and
  observed application state (conceptually
  InSync/Missing/Unexpected/Different/Unknown).
- Reconciliation decisions driven by observation of the world, not only by the
  outcome of Pneuma's previous operation.
- Unknown-state handling: Unknown/Unobservable/partially observed results stay
  legitimate outcomes instead of being forced into healthy/failed.

## Boundaries

- Repair/recovery robustness (idempotent operations, crash recovery, retry
  policy) belongs to v0.7; v0.6 only observes and reports divergence.
- Multi-service applications belong to v0.8.
- No new product features beyond this scope before an approved design defines
  entities, persistence, runtime behavior, and acceptance scenarios.

See [`../roadmap.md`](../roadmap.md) for the authoritative v0.6 scope.
