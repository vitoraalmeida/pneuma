# Next Iteration

**Status:** planning reminder, not an execution tracker.

**Target:** v0.5 - Application Topology and Internal Networking.

Do not implement this work until v0.4 is complete, an approved design exists,
and this tracker is promoted to `current-iteration.md`.

## Objective

Model how Applications relate to each other so Pneuma can support internal
services and their connectivity rather than treating every Application as an
isolated workload.

## Checkpoints

- Service relationships and Application dependencies.
- Internal services and network/service addressing.
- System as a functional grouping mechanism.
- Basic service discovery.

## Boundaries

- Network policy enforcement belongs to v0.6.
- Workload identity and authenticated service-to-service communication belong to
  v0.7.
- Do not introduce topology before an approved design defines entities,
  persistence, runtime behavior, and acceptance scenarios.

See [`../roadmap.md`](../roadmap.md) for the authoritative v0.5 scope.
