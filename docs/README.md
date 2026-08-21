# Pneuma Documentation

## Documentation Model

Each category answers a different question.

| Type | Question | Documents |
|---|---|---|
| Current system truth | How does Pneuma work today? | [`architecture/`](architecture/), [`getting-started.md`](getting-started.md), [`operations/`](operations/) |
| Architectural rationale | Why was this choice made? | [`decisions/`](decisions/) |
| Approved future design | How will a decided, unimplemented feature work? | [`design/`](design/) |
| Active planning | What is being implemented now? | [`iterations/current-iteration.md`](iterations/current-iteration.md) |
| Product evolution | Where is Pneuma going? | [`roadmap.md`](roadmap.md) |
| Released history | What changed in releases? | [`../CHANGELOG.md`](../CHANGELOG.md) |

**Living** documents describe current behavior and change with it. An **approved
design** describes decided future behavior. **Active planning** tracks execution,
not system truth. A **historical record** preserves what was completed or
released and is not rewritten to make history appear different.

When documents disagree, trust current architecture first, then an approved
design for future behavior, then the active iteration tracker for progress. The
roadmap describes direction and history; the changelog records releases. Neither
overrides current architecture.

## Reader Journeys

| Intent | Read in order |
|---|---|
| Understand Pneuma | [`../README.md`](../README.md) → `architecture/system-context.md` → `architecture/architecture.md` → `architecture/data-model.md` → `architecture/security-model.md` → `architecture/threat-model.md` → relevant ADR |
| Deploy Pneuma | [`getting-started.md`](getting-started.md) → [`operations/`](operations/) for disposable VM validation |
| Contribute | system context → architecture → [`rust-guidelines.md`](rust-guidelines.md) → active iteration → relevant approved design |
| Understand future work | [`roadmap.md`](roadmap.md) → relevant `design/` document → active iteration |

## Operating Environments

| Environment | Use | Entry point |
|---|---|---|
| Production host | Bootstrap, application operation, and non-destructive smoke tests only | [`getting-started.md`](getting-started.md) |
| Development VM | Disposable Debian 13 provisioning and manual regression | [`operations/dev-vm-tutorial.md`](operations/dev-vm-tutorial.md) |
| Automated VM regression | Fixture-cycle E2E and full disposable regression | [`../scripts/dev-vm/e2e.sh`](../scripts/dev-vm/e2e.sh), [`../scripts/dev-vm/test-all.sh`](../scripts/dev-vm/test-all.sh) |

Do not run reset, restore, bootstrap acceptance, or E2E scripts on production.

## Documentation Requirements

For every change, determine whether it changes implemented behavior
(`architecture.md`), persistence (`data-model.md`), trust boundaries
(`security-model.md`), architecture threats (`threat-model.md`), a major decision
(ADR), future behavior (design), user setup (`getting-started.md`), released
behavior (`CHANGELOG.md`), or product direction (`roadmap.md`).

## Index

| Document | Status | Contents |
|---|---|---|
| [`rust-guidelines.md`](rust-guidelines.md) | Living | Mandatory Rust code conventions |
| [`getting-started.md`](getting-started.md) | Living | Debian 13 VPS setup, application delivery, and reference |
| [`operations/dev-vm-tutorial.md`](operations/dev-vm-tutorial.md) | Living | Disposable Debian 13 VM provisioning and E2E procedure |
| [`architecture/system-context.md`](architecture/system-context.md) | Living | Problem, intended environment, goals, constraints, and vocabulary |
| [`architecture/architecture.md`](architecture/architecture.md) | Living | Implemented architecture, authority boundaries, rules, and flows |
| [`architecture/data-model.md`](architecture/data-model.md) | Living | Implemented SQLite model and persistence invariants |
| [`architecture/security-model.md`](architecture/security-model.md) | Living | Current assets, trust boundaries, controls, and security limits |
| [`architecture/threat-model.md`](architecture/threat-model.md) | Living | Architecture threats, attack paths, assumptions, and residual risks |
| [`decisions/`](decisions/) | Historical records | Retrospective architectural decision records |
| [`design/documentation-architecture-refactor.md`](design/documentation-architecture-refactor.md) | Approved design | Current documentation-refactor design |
| [`design/reconciliation.md`](design/reconciliation.md) | Approved design | Queued v0.4 reconciliation semantics |
| [`design/reconciliation-e2e.md`](design/reconciliation-e2e.md) | Approved design | Queued v0.4 E2E catalog |
| [`design/domain-model-hardening.md`](design/domain-model-hardening.md) | Approved design | v0.4 domain-model hardening before reconciliation |
| [`design/domain-boundary-refactor.md`](design/domain-boundary-refactor.md) | Approved design | v0.4 domain, store, and use-case boundary alignment |
| [`design/caddy-unmatched-host-fallback.md`](design/caddy-unmatched-host-fallback.md) | Historical record | Completed v0.3.1 design |
| [`iterations/current-iteration.md`](iterations/current-iteration.md) | Active planning | v0.4 reconciliation and boundary-refactor tracker |
| [`iterations/next-iteration.md`](iterations/next-iteration.md) | Queued planning | v0.5 topology planning reminder |
| [`roadmap.md`](roadmap.md) | Living | v0.1 → v0.8 evolution and direction |
