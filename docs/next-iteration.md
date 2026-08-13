Before starting to implement v0.3, I would carry out a formal v0.2 consolidation stage. This is not about reshaping Pneuma; it is about making the existing model explicit, consistent, and sufficiently tested so the reconciler is not born on top of ambiguities.

The current state is already a solid foundation: v0.2 is marked complete, CI → OCI → SSH → ci dispatch → deploy works, bootstrap has been validated on a clean Debian 13 installation, and the current suite records 27 PASS / 0 FAIL / 1 SKIP.

The pre-v0.3 objective

At the end of this stage, these statements must be true:

Release
= immutable artifact that can be deployed

Deployment
= attempt/event to activate a Release

Runtime
= concrete materialization produced by a Deployment

Application
= owns desired state and points to the active Deployment

SQLite
= intent + history

Podman/systemd/Caddy
= observed state

Use case
= decision/orchestration

Store
= persistence

CI
= only requests operations from Pneuma

Reconciler
= does not exist yet, but its rules are already defined

Today, the general model already points in this direction: the roadmap itself defines Release as immutable, Deployment as an attempt, and differentiates desired from observed state.

1. Fix the roadmap before any v0.3 code

This would be the first task.

The current roadmap.md still calls v0.3 “Reconciliation, automation & CI/CD” and presents GitHub Actions, build/push to GHCR, SSH deployment, and forced command as pending. This no longer represents the current state.

I would change it to:

v0.1 — OCI Deployment Foundation         ✅
v0.2 — Git-aware OCI Delivery            ✅

v0.3 — Reconciliation & Deployment Reliability
v0.4 — Application Topology & Internal Networking
v0.5 — Network Policy Enforcement
v0.6 — Workload Identity & Secure S2S
v0.7 — Artifact Security & Secrets

And I would remove the following from v0.3 as future work:

GitHub Actions build/push
SSH deployment
forced command
CI dispatcher
registry watcher
automatic deploy policies
dedicated pneuma-deployer

The first four already exist; registry watcher and auto-deploy policies have no demonstrated need in the current model; and the restricted identity has already been implemented using the pneuma user itself.

I would also reframe:

automatic rollback on health failure

because a candidate failing before promotion differs from performing an automatic rollback after a promotion.

2. Formally close v0.2 and open a “pre-v0.3 consolidation” iteration

I would not start the next current-iteration.md with:

Implement Reconciler.

I would create something like:

Pre-v0.3 — Domain and persistence consolidation

It should have the following Definition of Done:

roadmap updated
Release/Deployment/Runtime domain consolidated
mutable SQL removed from the main use cases
no deployment regressions
deployment concurrency validated
reconciliation semantics documented
clean E2E passing

The current iteration already records bootstrap, the stop/start/status cycle, the CI dispatcher, and the complete regression as complete.

3. Definitively consolidate Release, Deployment, and Runtime

This is probably the most important conceptual adjustment before the reconciler.

The database/model is correct, but the types are distributed inconsistently.

Today there is:

domain/
└── release.rs
      Release

use_cases/
└── deployment_create.rs
      Deployment
      DeploymentType
      DeploymentStatus
      RuntimeState

Deployment, DeploymentType, DeploymentStatus, and RuntimeState are central domain concepts, but they currently live within a use case.

I would transform it into:

domain/
├── release.rs
│     └── Release
│
├── deployment.rs
│     ├── Deployment
│     ├── DeploymentType
│     └── DeploymentStatus
│
└── runtime.rs
      └── RuntimeState

No sophisticated behavior, traits, or new abstractions. Just correct ownership of the concepts.

Eliminate the duplicate Release

Today there is:

domain::release::Release

and also:

adapters::stores::release_store::Release

with practically the same fields.

I would eliminate the latter.

The store should simply return:

domain::release::Release

This makes explicit that there is a single conceptual representation of Release.

4. Fix the names that currently blur Release and Deployment

The main example is:

pub struct DeployedRelease {
    pub deployment_id: String,
    pub runtime_id: String,
    ...
}

This object represents the result of a Deployment, but is named DeployedRelease.

I would rename it to something like:

DeploymentResult

That would be my preference, because the function:

deploy_release(...)

would clearly mean:

input:
Release

operation:
Deployment

output:
DeploymentResult

This reinforces:

Release ≠ Deployment

without changing any behavior.

5. Fix the source_revision ambiguity

There is currently:

let runtime_identity =
    release.source_revision.as_deref().unwrap_or(&release.id);

That is, if there is no Git commit, a release.id is used in the position of an identity derived from source_revision.

I would not let this enter v0.3 as-is.

The question must be:

What does this string actually exist for in the runtime?

If it is a label to identify the running version, it should have a name such as:

runtime_revision
artifact_identity
release_identity

and use a coherent identity, probably:

source_revision when available
or
image_digest

But release.id should not semantically become a source revision.

This needs to be resolved before the reconciler because reconciliation will need to identify precisely:

“Does this observed runtime correspond to the Release that should be active?”

6. Finish separating use cases and stores

This is the largest structural task of pre-v0.3.

The v0.2 roadmap itself defines the rule:

use cases decide what should happen; SQLite stores decide how to persist.

But this is not yet applied uniformly.

application_import

Today it opens a transaction and directly contains SQL to:

create/find System;
create Application;
delivery spec;
source spec;
runtime spec;
health check;
exposure.

Conceptually, it should become:

application_import
    │
    ├── validates manifest
    ├── defines intent
    │
    └── transaction
          │
          └── application_store
                ├── ensure_system
                ├── insert_application
                ├── insert_delivery_spec
                ├── insert_source_spec
                ├── insert_runtime_spec
                ├── insert_health_spec
                └── insert_exposure

The transaction remains in the use case when it needs to guarantee atomicity for the set.

This is important: we do not want to simply “hide transactions in stores”.

application_runtime

This is even more important because it will be reused directly by the reconciler.

Today it has its own queries to load desired state/current runtime and its own updates for desired state and runtime observations.

I would move these operations to:

application_store
runtime_store

leaving application_runtime with only:

load intent
        ↓
observe Podman
        ↓
decide
        ↓
produce effect
        ↓
persist result

This is almost exactly the pattern the reconciler will need.

exposure_change

Today it also directly contains reads and writes to the Applications, exposures, and runtimes tables.

I would move the queries to stores before the reconciler, because exposure will be part of future desired-vs-observed.

deployment_create

This is already partially better: it uses application_store, release_store, and deployment_store, but it still retains its own SQL to check for a concurrent deployment, discover the active Release, insert a Deployment, and reload it.

I would finish the migration.

The result should be:

deployment_create
      │
      ├── opens TransactionBehavior::Immediate
      ├── checks rules
      ├── calls stores
      └── commit

and contain no SQL.

The final rule

I would write explicitly in architecture.md:

Use case owns:
- orchestration
- business decisions
- ordering
- transaction boundary when atomicity spans multiple writes

Store owns:
- SQL
- mapping database ↔ domain
- persistence primitives

External adapters own:
- Git
- OCI
- Podman
- systemd
- Caddy

And I would retain a rule the current architecture already has:

Never keep a SQLite transaction open during Git, Podman, Caddy, or HTTP.

7. Do not create additional abstractions during this cleanup

This also needs to be part of the plan.

I would not add before v0.3:

Repository<T>
Repository traits
UnitOfWork
Service layer
DI container
generic storage abstractions
async
event bus

The existing concrete stores are sufficient.

The cleanup is:

SQL left the use case

and not:

let's redesign the entire persistence architecture

This retains the simple style the project explicitly seeks; the current architecture even records the deliberate absence of traits/generics/async for these abstractions.

8. Validate the existing mutual exclusion

I would not implement new locking before v0.3.

create_deployment() already opens:

TransactionBehavior::Immediate

and within it checks whether another non-terminal deployment for the Application exists before inserting the next one.

This is a good basis to prevent:

deploy A
+
deploy A

from running concurrently.

What is missing is proving this with a real concurrency test, ideally two separate processes/connections.

The test should guarantee:

process 1
pneuma app deploy app ...

simultaneous process 2
pneuma app deploy app ...

result:

1 wins
1 receives ActiveDeployment

If this passes, we would mark:

deployment mutual exclusion ✅

instead of building another lock.

Later, for v0.3, we will need to define the interaction:

deploy × reconcile

but that first depends on the reconciler design.

9. Do not change CI/bootstrap further, except for validation/documentation

This block can already be considered essentially complete.

The current bootstrap creates the operational environment, uses /etc/pneuma/environment, and already supports the restricted CI identity; architecture.md records ci_dispatch as a forced-command that accepts only deploy <app> <branch> and version.

The current iteration also records staging working through this restricted CI identity.

Therefore, before v0.3:

no:
  redesign SSH
  create pneuma-deployer
  create API
  add OIDC

yes:
  keep tests
  update roadmap
  validate that production continues to work
10. Write the reconciliation design before reconciler.rs

This is the last major mandatory artifact before starting v0.3 code.

I would create:

docs/design/reconciliation.md

or an equivalent name.

This document needs to freeze semantics, not implementation.

Sources of truth

I would define:

SQLite
    desired state
    active deployment
    Release identity
    history

Podman/systemd
    observed runtime state

Caddy
    observed exposure state

OCI registry
    artifact availability

The current architecture already makes it explicitly clear that SQLite is not the source of observed runtime state; Podman is.

Runtime matrix

For example:

Desired	Observed	Action
Running	Running	no-op
Running	Stopped	start
Running	Missing	recover
Running	Failed	recover/report
Stopped	Running	stop
Stopped	Stopped	no-op
Stopped	Missing	no-op

Part of this semantics already exists today: the current suite shows that a removed container with desired Running can be recreated by Quadlet and that status reconciles observed identity; there was also a specific fix for Stopped + Missing.

Exposure matrix
desired Public
observed correct
→ no-op

desired Public
fragment missing
→ materialize

desired Public
fragment wrong
→ replace

desired Internal
fragment missing
→ no-op

desired Internal
fragment present
→ remove
Deployment recovery

Define what happens if the process dies in:

Pending
Starting
Verifying
Activating

My initial policy would remain conservative:

do we not know whether promotion completed?
        ↓
do not promote automatically
        ↓
inspect real state
        ↓
preserve previous healthy runtime
        ↓
safe cleanup
        ↓
mark interrupted/failed
Invariants

I would document at least:

an Application has at most one active Deployment

a Release is immutable

reconciliation does not create a new Release

runtime recovery does not create a new Deployment by default

reconciliation does not change desired state

reconciliation never selects a newer version

reconciliation does not observe the registry looking for a new release

reconciliation must be idempotent

This last sentence defines much of v0.3:

Reconcile does not decide a new intent; it only converges reality toward an already persisted intent.

11. Define the v0.3 test suite before implementation

It is not necessary to write all test code in advance, but the scenarios must be defined.

The test document should cover four classes:

RUNTIME DRIFT

kill container
remove container
stop unit
remove Quadlet materialization
reboot


EXPOSURE DRIFT

delete Caddy fragment
alter Caddy target
public without route
internal with route


DEPLOYMENT RECOVERY

crash in Pending
crash in Starting
crash in Verifying
crash in Activating


CONCURRENCY / IDEMPOTENCY

reconcile twice
parallel reconcile
deploy × deploy
deploy × reconcile

This way v0.3 becomes driven by expected behaviors, rather than the shape reconciler.rs takes.

12. Run a complete regression after consolidation

Only after the domain/persistence refactors.

First:

cargo fmt --check

cargo clippy \
  --all-targets \
  --all-features \
  -- \
  -D warnings

cargo test --all-features

cargo build --release

The current iteration already uses these four gates.

Then, clean Debian VM:

bootstrap
   ↓
doctor
   ↓
system create
   ↓
app import
   ↓
branch deploy
   ↓
candidate + health + promotion
   ↓
status
   ↓
stop
   ↓
start
   ↓
visibility
   ↓
rollback
   ↓
reboot
   ↓
CI dispatcher

The current baseline is strong: bootstrap 20 PASS / 0 FAIL and the main suite 27 PASS / 0 FAIL / 1 SKIP. Pre-v0.3 should not reduce this.

Concrete order I would follow

I would do it exactly in this sequence:

docs: redefine roadmap after v0.2 — fix v0.3 and split v0.5 into future stages.
docs: open pre-v0.3 consolidation iteration — record scope and DoD.
refactor(domain): make deployment and runtime first-class domain types — move Deployment, DeploymentType, DeploymentStatus, RuntimeState.
refactor(release): use a single domain Release type — eliminate release_store::Release.
refactor(deployment): rename DeployedRelease to DeploymentResult.
refactor(runtime): separate source revision from runtime identity — resolve the release.id fallback.
refactor(store): move application import persistence to application store.
refactor(store): move application runtime persistence to stores.
refactor(store): move exposure persistence to stores.
refactor(store): finish deployment create persistence extraction.
test(deployment): verify concurrent deployment exclusion.
docs: define reconciliation semantics and invariants.
test: define v0.3 reconciliation E2E scenarios.
complete regression on a clean VM.
Optionally publish v0.2.1 as a consolidated baseline.
Only then: first functional v0.3 commit.
What should not block v0.3

I would not wait for:

registry watcher
generic idempotency key
complete GitHub audit trail
image retention
HTTP API
TUI
OIDC
GitHub App
RBAC
new Linux deployment user

These are valid concerns, but none is required to correctly implement desired-vs-observed and recovery.

Definition of Done for pre-v0.3

I would only start pneuma reconcile when I can answer yes to these questions:

Is there a single, unambiguous definition of Release, Deployment, and Runtime in the code?

Are the main use cases the reconciler will reuse free of direct SQL?

Is it clear who controls transactions and who controls SQL?

Does source_revision mean only source revision?

Has concurrent Deployment already been tested?

Do we know exactly which system is the source of truth for each state?

Do we have a table saying what to do for every desired × observed combination?

Do we know what to do with every interrupted Deployment?

Do we explicitly know what the reconciler cannot do?

Does the entire v0.2 regression remain green?

When these answers are positive, the first v0.3 implementation can be small and very focused:

pneuma reconcile <app>
        ↓
observe
        ↓
compare
        ↓
produce ReconciliationPlan
        ↓
apply

This is the point at which I would consider the foundation truly ready to evolve from “Pneuma executes changes” to “Pneuma maintains desired state”
