# Next Iteration

**Status:** planning reminder, not an execution tracker.

**Target:** v0.5 - Application Topology & Internal Networking.

This document is a reminder of the roadmap work that follows v0.4
reconciliation. Do not implement it until v0.4 is complete, an approved design
exists, and `current-iteration.md` is replaced with the corresponding active
tracker.

## Objective

Model how Applications relate to each other so Pneuma can support internal
services and their connectivity, rather than treating every Application as an
isolated workload.

## Roadmap Scope

- Service relationships: Application A depends on Application B.
- Internal services.
- Application dependencies.
- Network and service addressing.
- System as a real grouping mechanism.
- Basic service discovery.

## Boundaries

- This iteration does not define host network enforcement; that belongs to v0.6.
- Workload identity and authenticated service-to-service communication belong to
  v0.7.
- Do not introduce a topology implementation before a v0.5 approved design
  defines entities, persistence, runtime behavior, and acceptance scenarios.

See [`../roadmap.md`](../roadmap.md) for the authoritative v0.5 scope.
