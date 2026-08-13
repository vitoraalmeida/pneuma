# Current Iteration

**Status:** in progress

**Base:** `ce8b18f` (`docs: complete architecture audit`)

**Approved design:**
[`documentation-architecture-refactor.md`](../design/documentation-architecture-refactor.md)

## Iteration - Documentation Architecture Refactor

Objective: make current behavior, rationale, trust boundaries, future design,
active work, and historical evolution distinct and navigable before v0.4
reconciliation implementation begins.

## Checkpoints

- [x] Establish taxonomy, precedence, reader journeys, tracker transition, and
  metadata correctness.
- [x] Add system context, vocabulary, implemented-architecture navigation,
  invariants, and end-to-end scenarios.
- [x] Add the ADR mechanism and retrospective decisions for platform, delivery,
  runtime, state, exposure, domain, and CI-dispatch architecture.
- [x] Add the current security model and trust-boundary documentation.
- [x] Refocus the root README and retain detailed CLI, manifest, and configuration
  reference in the getting-started guide.
- [x] Separate roadmap history from current architecture and preserve completed
  detail under `roadmap-history/`.
- [x] Align terminology and cross-links; add local Markdown-link CI validation;
  complete the new-engineer documentation review.

Validation: local Markdown-link, shell, format, clippy, test, and release-build
checks pass. Rootless Podman tests remain ignored because this host is not
configured for that environment; no VM regression was required by this
documentation-only iteration.

## Scope and Non-goals

- This iteration changes documentation and documentation validation only.
- It does not implement reconciliation or alter v0.4 behavior.
- It does not rewrite release-history records to use current terminology.
- It does not create a documentation platform or per-technology manuals.

## Acceptance Criteria

- Documentation precedence and reader journeys identify the correct authority.
- Current architecture, rationale, security, operations, future design, roadmap,
  and release history have distinct homes and cross-links.
- The exact CI gates, including local Markdown-link validation, are green.
- The new-engineer review questions can be answered without source inspection.

## Blockers

None.
