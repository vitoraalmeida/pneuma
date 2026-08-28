# ADR-0003 — Health-gated activation

**Status:** Accepted

## Context

An artifact, an activation attempt, and a materialized runtime have distinct
lifecycles. Replacing a live route before candidate health is proven risks an
otherwise healthy Application.

## Decision

Release, Deployment, and RuntimeInstance remain separate. Rollback creates a
new Deployment. Promotion is health-gated and atomically records active
persisted state. Candidate failure preserves the prior active runtime and route.
Exposure intent remains independent from runtime lifecycle.

## Consequences

Deployment history is append-only. Public promotion also requires confirmed route
materialization and route health; it never treats desired visibility as route
evidence.
