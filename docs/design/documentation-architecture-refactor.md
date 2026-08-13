# Design - Documentation Architecture Refactor

**Status:** approved design. It defines the documentation refactor that precedes
v0.4 reconciliation; execution and progress live in
[`../iterations/current-iteration.md`](../iterations/current-iteration.md).

## Objective

Make Pneuma understandable in a predictable order: why it exists, the
constraints that shape it, implemented behavior, architectural rationale, trust
boundaries, future design, and product direction.

## Decisions

- `architecture/` describes current system truth. `system-context.md` explains
  motivation and constraints; `architecture.md` explains implemented behavior;
  `data-model.md` explains persistence; `security-model.md` explains trust
  boundaries.
- `decisions/` contains concise retrospective ADRs for decisions already
  embodied in the implementation. ADRs explain why; architecture explains what.
- `design/` contains approved but unimplemented behavior. A completed design is
  a historical record and is not rewritten as current truth.
- `iterations/current-iteration.md` is the sole active execution tracker.
  Queued work is recorded separately and is not an implementation authority.
- `roadmap.md` records evolution and direction. Completed detailed plans move to
  `roadmap-history/`; neither replaces current architecture.
- The root README is a five-minute newcomer guide. Detailed CLI, manifest, and
  configuration reference stays in the getting-started guide.
- CI checks local relative Markdown link paths only. It does not attempt external
  URL, anchor, or semantic-documentation validation.

## Non-goals

- Do not change product behavior, persistence, deployment semantics, or security
  controls.
- Do not split documentation into per-technology manuals.
- Do not rewrite CHANGELOG entries to make historical wording match current
  terminology.
- Do not automate semantic synchronization between documents.

## Acceptance Criteria

- Readers can identify the authority for implemented behavior, approved future
  design, active work, roadmap direction, and released history.
- Current documentation consistently distinguishes Release, Deployment,
  RuntimeInstance, desired state, observed state, runtime, and exposure.
- The root README links readers to the appropriate detail without duplicating it.
- Every local relative Markdown link resolves in CI.
- A newcomer can answer the documented product, delivery, runtime, persistence,
  exposure, security, and evolution questions without source inspection.
