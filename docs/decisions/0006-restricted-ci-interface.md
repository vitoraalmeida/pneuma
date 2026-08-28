# ADR-0006 — Restricted CI interface

**Status:** Accepted

## Context

CI needs a deployment capability but must not receive arbitrary shell access to
the host running workloads.

## Decision

The CI SSH identity receives a forced restricted command, not a shell. Accepted
arguments pass current domain validation and dispatch through normal use cases.

## Consequences

The restricted interface has the same deployment validation and orchestration as
local CLI use; it does not directly invoke host adapters or persistence.
