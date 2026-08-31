# Pneuma Core Concepts

**Status:** living document - describes the domain vocabulary as implemented in v0.4.3.

This guide gives a new contributor the minimum vocabulary needed before reading
use cases. It explains what each core term means and how the terms relate;
[`architecture/architecture.md`](architecture/architecture.md) describes how the
system behaves, and [`code-guide.md`](code-guide.md) maps flows through the code.
Terms below are capitalized when they name a domain concept that appears in the
code.

## Application and System

An **Application** is the durable, command-facing identity of one deployed
workload. It is created once by an import and survives deployments, rollbacks,
and restarts. The import validates a `pneuma.toml` manifest — delivery,
runtime, health, and exposure configuration — and stores it as the
Application's immutable specification. Import is create-only: repeating it
returns the existing Application without rewriting anything.

An Application always has one desired runtime state (`running` or `stopped`),
one active deployment once a first promotion succeeds, and exactly one
Exposure record. A **System** is an organizational grouping for Applications.
Import requires selecting a System (by flag or from the manifest); Pneuma
creates the named System when it does not exist yet.

## Release

A **Release** is a reusable, immutable OCI artifact belonging to one
Application: a repository plus a `sha256:` digest (`repository@sha256:<hex>`).
Mutable tags never become Release identity; a new artifact digest is a new
Release, and an existing `(application, digest)` pair is reused rather than
duplicated. An Application also records which OCI repository its manifest
declared, and only artifacts from that repository may ever be deployed for it.

## Source revision versus OCI digest

Pneuma resolves source code to an artifact in one direction:

```text
branch or tag → Git commit SHA → repository:<commit-sha> tag → resolved digest
```

The commit SHA is the **source revision**. It is recorded on the Deployment,
not on the Release, because the same artifact can be activated by different
requests (for example, redeploying the same digest from different branches).
The Release keeps only the resulting digest. If CI has not published an image
tagged with the requested commit, deployment fails; Pneuma never falls back to
`latest`, an older artifact, or a local build.

## Deployment

A **Deployment** is one attempt to activate a Release for an Application. Each
attempt is append-only history: rows are never edited into something else, and
a rollback creates a new Deployment of type `rollback` pointing at a historical
Release rather than rewinding any prior row. An Application may hold at most
one non-terminal Deployment at a time.

A Deployment walks a fixed status sequence:

```text
pending → starting → verifying → activating → succeeded   (public)
pending → starting → verifying →             succeeded   (internal)
any non-terminal status → failed
```

`succeeded` and `failed` are terminal. A failure persists durable evidence — a
code, the stage where it happened, a message, and a timestamp — so history
explains itself without external inspection.

## Runtime

A **RuntimeInstance** is the logical record of the concrete runtime a
Deployment materialized: its reserved loopback endpoint, its container port,
and the last confirmed observation of the outside world. Logical identity is
deliberately separate from external identity. The Podman container ID can
change (Quadlet recreates containers after host reboots), while the
RuntimeInstance stays the same logical thing. All three external names derive
from one stable base name, `pneuma-<application>-<deployment-id>`: the
container, the Quadlet file `<base>.container`, and the systemd service
`<base>.service`.

Every runtime binds to IPv4 loopback only — `127.0.0.1:<reserved-port>`, with
the port taken from the configured runtime port range — even for public
Applications. Caddy, not port publishing, provides public reachability.

Two state vocabularies must not be confused:

- **Recorded runtime state** is what Pneuma believes (`starting`, `running`,
  `stopped`, `failed`). Pneuma writes it after confirming effects.
- **Observed runtime state** is what Podman last reported. Unrecognized status
  text is preserved as explicitly unknown instead of being forced into a known
  value, so observation never invents facts.

## Candidate and current runtime

While a Deployment is non-terminal, its runtime is a **candidate**: registered
with recorded state `starting`, health-checked on its loopback endpoint before
anything switches. A candidate that fails its checks is cleaned up — unit,
container, reservation — but only resources proven to belong to it, and the
previously active runtime and route stay untouched.

**Promotion** is the transactional act of confirming a healthy candidate: the
Deployment becomes terminal `succeeded`, its recorded runtime state becomes
`running`, and the Application's active deployment pointer moves to it. After
promotion the candidate is the **current (active) runtime** — the code and docs
say *active*. Retirement of the prior runtime happens afterward and is best
effort; a retirement warning never undoes a completed promotion.

## Exposure

An **Exposure** pairs one Application's visibility intent with the confirmed
state of its public route. Intent is `internal` or `public`; public intent
requires a validated domain name. Materialization is tracked separately:
`not_materialized`, `applying`, `active`, `removing`, `failed`, or `diverged`.
Changing intent alone activates nothing — a route becomes `active` only after
Caddy accepts the fragment, reloads, and an external health check through the
public URL passes. An active route is tied to a specific runtime and a specific
fragment version, so confirmation proves the published configuration matches
what was intended.

When materialization or confirmation fails, Pneuma attempts to restore the
previous fragment. If compensation completes, the state is `failed`; if
compensation itself cannot be confirmed, the state is `diverged` and demands
manual inspection. Stopping a public Application does not remove its route.

## Desired, persisted, and observed state

Three categories of state run through everything:

- **Desired state** is operator intent persisted in SQLite: the runtime state
  the Application should converge to, and the visibility the Exposure should
  have.
- **Persisted state** more broadly is everything SQLite owns: desired intent,
  imported specification, logical identities, deployment history, and last
  confirmed results.
- **Observed state** is what external authorities report right now: Podman and
  systemd for containers and units, Caddy for routes.

Persisted intent is never treated as an observation. SQLite does not prove that
a container still exists; use cases observe authorities before claiming
results, persist intent before effects, and persist confirmed completion after
observing them. Writes that confirm effects are compare-and-set guarded, and a
zero-row update means concurrent or stale state — never success.

## Reconciliation

**Reconciliation** is the on-demand convergence pass behind
`pneuma reconcile <application>`. It loads desired and persisted facts in a
short transaction, closes it, then observes Podman, systemd, and Caddy. A pure
domain decision compares three inputs — desired intent, persisted bookkeeping
(such as a blocking non-terminal Deployment or the active bundle), and the
observation snapshot — plus canonical expectations rendered at the boundary
(what the container name, Quadlet bytes, and route fragment *should* look
like). The outcome is one explicit action: no-op, deferred, runtime repair,
exposure repair, or a recorded failure or divergence with a reason. Drift no
safe rule covers becomes manual intervention; unknown external states stay
unknown rather than becoming invented stopped-or-running facts.

Reconciliation is also the recovery path for interrupted work: if a lock holder
died mid-deployment, reconcile records the non-terminal Deployment failed and
cleans the candidate only when its persisted and external identity can be
proven.

## Reading further

[`architecture/architecture.md`](architecture/architecture.md) covers behavior
and authority boundaries, [`architecture/data-model.md`](architecture/data-model.md)
the persisted schema, and [`architecture/invariants.md`](architecture/invariants.md)
the durable-guarantee inventory. [`code-guide.md`](code-guide.md) traces
each user-facing flow through the layers.
