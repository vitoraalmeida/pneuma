# ADR-0002 — Immutable OCI delivery

**Status:** Accepted

## Context

A host deployment tool must not make delivery identity depend on a mutable tag
or a host-local build.

## Decision

CI builds and publishes artifacts; Pneuma never builds on the host. Requested
Git revisions resolve to immutable commits. A Release is a verified,
digest-pinned OCI artifact from the Application's configured repository.

## Consequences

Branch names and tags are resolution inputs, not Release identities. Repeating
selection of the same permitted artifact reuses its Application Release.
