# ADR-0001 — Single-host runtime

**Status:** Accepted

## Context

Pneuma deploys workloads for one Linux host. It does not need a resident control
plane to keep a promoted workload alive.

## Decision

Pneuma is an on-demand CLI. Rootless Podman and systemd Quadlet own long-lived
runtime supervision. Workloads bind only to loopback, and Caddy is the current
ingress adapter for public traffic.

## Consequences

There is no daemon, scheduler, or multi-host coordination layer. A command
records intent and confirmed results, then exits; systemd and Podman continue
supervision independently.
