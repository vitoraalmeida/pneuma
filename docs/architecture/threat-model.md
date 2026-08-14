# Pneuma Threat Model

**Status:** living document - describes architecture-level threats, assumptions,
and residual risks for the implemented single-host model.

This document complements [`security-model.md`](security-model.md). The security
model defines assets, trust boundaries, and controls; this threat model explains
how those boundaries can fail or be abused. It does not replace an application-
specific threat model for workloads deployed by Pneuma.

## Security Assumptions

- The host administrator and host root account are trusted.
- The Application repository and CI workflow are trusted to build the intended
  source revision.
- The OCI registry is trusted to serve the artifact selected during deployment.
- Application containers are less trusted than the host, but Pneuma is not a
  hostile multi-tenant platform.
- Linux user namespaces, rootless Podman, systemd, Caddy, SSH, and SQLite enforce
  their documented boundaries correctly.
- DNS and certificate lifecycle are operated outside Pneuma.

Breaking an assumption does not necessarily bypass Pneuma's validation. It can
instead cause Pneuma to safely and reproducibly deploy an artifact or state that
the operator did not intend.

## Threat Actors

| Actor | Relevant capability |
|---|---|
| Internet attacker | Sends requests to a public Application and Caddy. |
| Stolen CI-key holder | Requests `version` or deployment through the forced-command dispatcher. |
| Repository or CI attacker | Changes source, revisions, workflows, or published commit-tagged images. |
| Registry attacker | Changes mutable tags or makes digest-addressed content unavailable. |
| Compromised workload | Controls one Application container and resources or credentials available to it. |
| Local `pneuma`-account attacker | Controls deployment state and files writable by the deployment identity. |
| Host root attacker | Controls all host state and defeats Pneuma's local boundaries. |

## Architecture Threats

| Threat | Potential damage | Existing mitigation | Residual risk |
|---|---|---|---|
| Repository or CI compromise | Publish and deploy malicious application code. | Git revision resolves to a full commit SHA; the selected artifact becomes a digest-pinned Release. | Pneuma does not verify image signatures, attestations, or builder provenance. |
| Registry tag replacement before selection | Substitute content at `repository:<commit-sha>` before Pneuma resolves its digest. | Pneuma records and subsequently uses the resolved digest. | Digest pinning proves identity after selection, not who published the selected content. |
| Stolen CI deployment key | Repeatedly deploy an Application, select an unintended valid revision, or cause availability loss. | SSH forced command permits only `version` and `deploy`; deployment still enforces the imported repository. | One key can request deployment for any imported Application and syntactically valid branch or tag; there is no per-key Application or revision policy. |
| Public Application compromise | Steal Application data or credentials, attack reachable services, and consume host resources. | Rootless Podman and loopback binding reduce host privilege and direct public exposure. | Rootless containers are not a hostile-workload boundary; kernel/runtime escape and unrestricted reachable dependencies remain possible. |
| Resource exhaustion | One workload consumes CPU, memory, processes, disk, or ports and disrupts other Applications. | systemd supervises runtime failures. | Generated Quadlet units do not currently define per-Application resource quotas; all workloads share one host. |
| `pneuma` Unix-account compromise | Modify SQLite state, Quadlet units, managed Caddy fragments, checkouts, and deployment operations. | The account has no sudo and uses rootless Podman. | Deployment control is intentionally concentrated in one local identity. |
| Host root compromise | Control every workload, route, credential, database, and binary. | Standard Linux host controls are the outer boundary. | Pneuma cannot defend the host after root compromise. |
| Single-host failure | Make all Applications and deployment control unavailable. | systemd and Quadlet restore promoted runtimes after an ordinary reboot; backup/restore protects logical state. | There is no failover, replication, or multi-host recovery. |
| Persisted/observed drift | SQLite can describe running or public intent while a runtime or route is missing or divergent. | Authorities are separated and status observes Podman rather than trusting SQLite alone. | Complete convergence and ambiguity handling are v0.4 reconciliation work. |
| Backup substitution | Restore structurally valid but attacker-modified intent and history. | Restore runs SQLite integrity checks and creates a pre-restore backup. | Integrity checking does not authenticate the backup producer or contents. |
| Weak deployment attribution | Make a deployment difficult to attribute to a person or repository workflow. | Deployment history records activation attempts and source revisions. | Shared deployment keys and the absence of a complete audit identity limit attribution. |
| DNS or Caddy compromise | Redirect traffic, break TLS, or expose an unintended route. | Pneuma validates managed fragments and Caddy configuration before reload. | DNS, certificates, the base Caddy configuration, and host administration remain trusted. |
| Health-check evasion | Promote malicious or semantically broken code that returns the expected status. | Internal and public health checks prevent promotion of an unavailable candidate. | HTTP health proves bounded reachability/status, not application correctness or safety. |

## Restricted CI-Key Boundary

A stolen CI key alone does not provide an interactive shell and does not directly
permit arbitrary `podman`, `systemctl`, Caddy, SQLite, or filesystem commands.
The dispatcher accepts only:

```text
version
deploy <application> <branch-or-tag>
```

The deployment request still resolves the revision in the imported Application
repository, enforces that Application's configured OCI repository, and activates
a digest-pinned Release. The key can nevertheless request deployment for every
imported Application, so it remains an integrity and availability credential.

## Primary Remote Attack Chain

The highest-impact realistic remote chain is a supply-chain compromise:

```text
compromise repository or CI
    -> publish malicious image as repository:<commit-sha>
    -> request deployment with the restricted CI key
    -> Pneuma resolves and pins the malicious digest
    -> candidate returns the expected health status
    -> Pneuma promotes the malicious Application
```

Pneuma can operate correctly throughout this chain. Digest pinning prevents the
selected artifact from changing afterward, but does not establish authorization,
publisher identity, or trustworthy build provenance.

## Compromised Container Boundary

Application compromise normally gives control over the Application process,
data, outbound connectivity, and any credentials deliberately made available to
that container. It does not automatically grant the restricted CI key, the
`pneuma` login identity, or host root.

Host impact can still occur through resource exhaustion, attacks on reachable
services, unsafe mounted data or credentials, or a Linux kernel/container-runtime
escape. Pneuma therefore treats rootless execution as privilege reduction, not a
guarantee that hostile workloads are safe.

## Priority Risk Reductions

1. Verify image signatures and CI provenance attestations before creating or
   activating a Release.
2. Scope CI credentials to specific Applications and allowed revisions.
3. Add CPU, memory, process, and storage limits to generated Quadlet units.
4. Add explicit Application network policy and constrain outbound connectivity.
5. Authenticate database backups before restore.
6. Record deployment audit identity beyond a shared SSH key.
7. Complete non-destructive reconciliation for unambiguous runtime and exposure
   drift.
8. Minimize credentials and writable host resources exposed to containers.

## Security Posture

The current architecture is intended for a trusted operator running trusted
workloads on one host. It should not be treated as a secure platform for hostile
Applications or mutually untrusted tenants.

## Related Documents

- [`security-model.md`](security-model.md) defines assets, controls, and trust
  boundaries.
- [`system-context.md`](system-context.md) defines intended scope and non-goals.
- [`architecture.md`](architecture.md) describes implemented behavior and
  authority boundaries.
- [`../decisions/0002-ci-builds-pneuma-deploys.md`](../decisions/0002-ci-builds-pneuma-deploys.md)
  explains artifact delivery.
- [`../decisions/0007-restricted-ssh-ci-interface.md`](../decisions/0007-restricted-ssh-ci-interface.md)
  explains the CI dispatcher boundary.
