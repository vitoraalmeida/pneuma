# Pneuma Documentation

## Documentation Model

Each category answers a different question.

| Type | Question | Documents |
|---|---|---|
| Current system truth | How does Pneuma work today? | [`architecture/`](architecture/), [`getting-started.md`](getting-started.md), [`operations/`](operations/) |
| Architectural rationale | Why was this choice made? | [`decisions/`](decisions/) |
| Approved design | What approved design fixed recent architecture scope? | [`designs/greenfield-architecture-simplification.md`](designs/greenfield-architecture-simplification.md) |
| Active planning | What is being implemented now? | Nothing is active: [`iterations/current-iteration.md`](iterations/current-iteration.md) is the concluded v0.4.3 tracker, and [`iterations/next-iteration.md`](iterations/next-iteration.md) queues v0.6 planning |
| Product evolution | Where is Pneuma going? | [`roadmap.md`](roadmap.md) |
| Released history | What changed in releases? | [`../CHANGELOG.md`](../CHANGELOG.md) |

**Living** documents describe current behavior and change with it. **Active
planning** tracks execution, not system truth. A **historical record** preserves
what was completed or released and is not rewritten to make history appear
different.

When documents disagree, trust current architecture first, then the roadmap for
future direction, then the active iteration tracker for progress. The
roadmap describes direction and history; the changelog records releases. Neither
overrides current architecture.

## Reader Journeys

The canonical newcomer path is:

```text
../README.md → architecture/system-context.md → core-concepts.md →
architecture/architecture.md → code-map.md → code-guide.md → target implementation
```

Each step has one job: what Pneuma is, why it exists, the vocabulary,
how it cooperates, where flows live in code, and how to trace them. The
journeys below are role-oriented views of that path.

| Intent | Read in order |
|---|---|
| Understand Pneuma | [`../README.md`](../README.md) → `architecture/system-context.md` → [`core-concepts.md`](core-concepts.md) → `architecture/architecture.md` → `architecture/data-model.md` → `architecture/security-model.md` → relevant ADR |
| Deploy Pneuma | [`getting-started.md`](getting-started.md) → [`operations/`](operations/) for disposable VM validation |
| Contribute | system context → [`core-concepts.md`](core-concepts.md) → architecture → [`code-map.md`](code-map.md) → [`code-guide.md`](code-guide.md) → [`rust-guidelines.md`](rust-guidelines.md) → iteration trackers under [`iterations/`](iterations/) |
| Understand future work | [`roadmap.md`](roadmap.md) → [`iterations/next-iteration.md`](iterations/next-iteration.md) |

## Task Shortcuts

Experienced readers can jump directly to a task:

| Question | Route |
|---|---|
| How does deploy work? | [`architecture/architecture.md`](architecture/architecture.md) deploy and promotion flows → [`code-map.md`](code-map.md) "Deploy from OCI" → `src/use_cases/deployment/execute.rs` with `candidate.rs`, `activation.rs`, `promotion/` |
| How does reconciliation work? | [`core-concepts.md`](core-concepts.md) Reconciliation → [`architecture/architecture.md`](architecture/architecture.md) Reconcile flow → `src/domain/reconciliation/decision.rs` → `src/use_cases/reconciliation/` |
| What does this domain term mean? | [`core-concepts.md`](core-concepts.md) |
| Where should a new rule live? | [`architecture/architecture.md`](architecture/architecture.md) Responsibilities and Ownership → [`architecture/invariants.md`](architecture/invariants.md) |
| How do I operate a host? | [`getting-started.md`](getting-started.md) → [`operations/dev-vm-tutorial.md`](operations/dev-vm-tutorial.md) |

## Operating Environments

| Environment | Use | Entry point |
|---|---|---|
| Production host | Bootstrap, application operation, and non-destructive smoke tests only | [`getting-started.md`](getting-started.md) |
| Development VM | Disposable Debian 13 provisioning and manual regression | [`operations/dev-vm-tutorial.md`](operations/dev-vm-tutorial.md) |
| Automated VM regression | One-command disposable regression, fixture-cycle E2E, and reconciliation catalog | [`../scripts/dev-vm/test-regression.sh`](../scripts/dev-vm/test-regression.sh) (standard path), [`../scripts/dev-vm/e2e.sh`](../scripts/dev-vm/e2e.sh), [`../scripts/dev-vm/test-all.sh`](../scripts/dev-vm/test-all.sh), [`../scripts/dev-vm/reconciliation-e2e.sh`](../scripts/dev-vm/reconciliation-e2e.sh) |
| Portable disposable VM E2E | Raw-QEMU Debian 13 harness, one command locally and on GitHub Actions (manual and `main`) | [`operations/dev-vm-tutorial.md`](operations/dev-vm-tutorial.md) (section 10), [`../scripts/vm/run-e2e.sh`](../scripts/vm/run-e2e.sh), [`../.github/workflows/e2e.yml`](../.github/workflows/e2e.yml) |

Do not run reset, restore, bootstrap acceptance, or E2E scripts on production.

## Documentation Requirements

For every change, determine whether it changes implemented behavior
(`architecture.md`), persistence (`data-model.md`), trust boundaries or threats
(`security-model.md`), a major decision
(ADR), user setup (`getting-started.md`), released behavior (`CHANGELOG.md`), or
product direction (`roadmap.md`).

## Index

| Document | Status | Contents |
|---|---|---|
| [`rust-guidelines.md`](rust-guidelines.md) | Living | Mandatory Rust code conventions |
| [`getting-started.md`](getting-started.md) | Living | Debian 13 VPS setup, application delivery, and reference |
| [`operations/dev-vm-tutorial.md`](operations/dev-vm-tutorial.md) | Living | Disposable Debian 13 VM provisioning and E2E procedure |
| [`architecture/system-context.md`](architecture/system-context.md) | Living | Problem, intended environment, goals, constraints, and vocabulary |
| [`core-concepts.md`](core-concepts.md) | Living | Domain concepts: applications, releases, deployments, runtimes, exposure, and reconciliation vocabulary |
| [`code-map.md`](code-map.md) | Living | Business-flow reading order: entry points, happy paths, branches, and failure paths per command |
| [`code-guide.md`](code-guide.md) | Living | Code navigation guide: each user-facing flow traced through CLI, use cases, domain, stores, adapters, and tests |
| [`architecture/architecture.md`](architecture/architecture.md) | Living | Implemented architecture, authority boundaries, rules, and flows |
| [`architecture/data-model.md`](architecture/data-model.md) | Living | Implemented SQLite model and persistence invariants |
| [`architecture/invariants.md`](architecture/invariants.md) | Living | Compact inventory of durable guarantees with their owner layers |
| [`architecture/security-model.md`](architecture/security-model.md) | Living | Assets, actors, trust boundaries, threats with their controls and residual risks, attack chains, and security posture |
| [`decisions/`](decisions/) | Historical records | Retrospective architectural decision records |
| [`designs/greenfield-architecture-simplification.md`](designs/greenfield-architecture-simplification.md) | Concluded design (implemented in v0.5.0) | Greenfield architecture reset scope, fixed decisions, and checkpoint order |
| [`iterations/current-iteration.md`](iterations/current-iteration.md) | Concluded planning | Greenfield architecture simplification execution tracker (all checkpoints done) |
| [`iterations/next-iteration.md`](iterations/next-iteration.md) | Queued planning | v0.6 observed state planning reminder |
| [`roadmap.md`](roadmap.md) | Living | v0.1 → v1.0 evolution and direction |
