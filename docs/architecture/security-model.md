# Pneuma Security Model

**Status:** living document - describes the current trust boundaries and
controls, and analyzes how those boundaries can fail or be abused. It is not a
claim that every deployment is secure against every threat, and it does not
replace an application-specific threat model for workloads deployed by Pneuma.

## Assets

- Ability to deploy Applications.
- Application repository identity and imported manifest configuration.
- OCI artifact identity.
- Pneuma SQLite intent and history.
- Managed Caddy routes.
- Active RuntimeInstance identity and host loopback endpoints.
- CI deployment credentials and host filesystem.

## Actors and Trust

| Actor | Trust relationship and relevant capability |
|---|---|
| Operator | Trusted host administrator who configures the host, repositories, DNS, and credentials. |
| Application repository | Trusted source of import-time configuration and requested Git revision; an attacker here can change source, revisions, workflows, or published commit-tagged images. |
| CI workflow | Trusted to build/publish the expected artifact and request deployment through its restricted key. |
| OCI registry | Trusted for the availability and bytes addressed by a resolved digest; an attacker here can change mutable tags or make digest-addressed content unavailable. |
| Internet client / attacker | Untrusted public client able to send requests to a public Application and Caddy. |
| Stolen CI-key holder | Can request `version` or deployment through the forced-command dispatcher, nothing else. |
| Application container | Less trusted than the host; controls its own process, data, outbound connectivity, and deliberately exposed credentials, but is not treated as a hostile-tenant boundary. |
| Local `pneuma`-account attacker | Controls deployment state and files writable by the deployment identity. |
| Host root attacker | Controls all host state and defeats Pneuma's local boundaries. |

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

## Trust Boundaries

The [system and trust-boundary diagram](system-context.md#system-context) shows
the external delivery, host, `pneuma` Unix-account, less-trusted container, and
public-ingress boundaries together.

Git and the OCI registry are external dependencies. SQLite, authorized keys,
systemd configuration, Caddy configuration, and the local host administrator are
trusted local boundaries. Root compromise defeats Pneuma's local controls. The
[authority map](architecture.md#authority-and-persistence) distinguishes
persisted intent from external observation.

## Threats and Controls

Each architecture-level threat, the control that addresses it, and what remains
after the control is applied:

| Threat | Damage if exploited | Control | Residual risk |
|---|---|---|---|
| Repository or CI compromise | Publish and deploy malicious application code. | Git revision resolves to a full commit SHA; the selected artifact becomes a digest-pinned Release. | Pneuma does not verify image signatures, attestations, or builder provenance; commit-SHA resolution assumes CI published `repository:<commit-sha>` correctly. |
| Registry tag replacement before selection | Substitute content at `repository:<commit-sha>` before Pneuma resolves its digest. | Pneuma records the resolved digest and subsequently uses only that digest-pinned Release. | Digest pinning proves identity after selection, not who published the selected content. |
| Registry digest bytes unavailable | Deployment or runtime recovery fails; availability loss. | Absence is reported explicitly; there is no fallback to `latest` or other artifacts. | Availability is external to Pneuma. |
| Stolen CI deployment key | Repeatedly deploy an Application, select an unintended valid revision, or cause availability loss. | SSH forced command permits only `version` and `deploy`; deployment still enforces each application's configured repository allow-list. | One key can request deployment for any imported Application and syntactically valid branch or tag; there is no per-key Application or revision policy. |
| Public Application compromise | Steal Application data or credentials, attack reachable services, and consume host resources. | Rootless Podman reduces host privilege; loopback binding prevents direct public access to runtime ports. | Rootless containers are not a hostile-workload boundary; kernel/runtime escape and unrestricted reachable dependencies remain possible, and host-local users and Caddy can still reach loopback. |
| Resource exhaustion | One workload consumes CPU, memory, processes, disk, or ports and disrupts other Applications. | systemd supervises runtime failures. | Generated Quadlet units do not currently define per-Application resource quotas; all workloads share one host. |
| `pneuma` Unix-account compromise | Modify SQLite state, Quadlet units, managed Caddy fragments, checkouts, and deployment operations. | The account has no sudo and uses rootless Podman. | Deployment control is intentionally concentrated in one local identity. |
| Persisted-state tampering | Desired intent, history, and logical identity are compromised. | SQLite write access requires the `pneuma` identity; authorities are separated and status observes Podman rather than trusting SQLite alone. | Any writer to the database file owns logical state. |
| Backup substitution | Restore structurally valid but attacker-modified intent and history. | Restore runs SQLite integrity checks and creates a pre-restore backup. | Integrity checking does not authenticate the backup producer or contents. |
| DNS or Caddy compromise | Redirect traffic, break TLS, or expose an unintended route. | Public routes exist only as managed Caddy fragments validated before reload, with an external health boundary. | DNS, certificates, the base Caddy configuration, and host administration remain operator-managed and trusted. |
| Host root compromise | Control every workload, route, credential, database, and binary. | Standard Linux host controls are the outer boundary. | Pneuma cannot defend the host after root compromise. |
| Single-host failure | Make all Applications and deployment control unavailable. | systemd and Quadlet restore promoted runtimes after an ordinary reboot; backup/restore protects logical state. | There is no failover, replication, or multi-host recovery. |
| Persisted/observed drift | SQLite can describe running or public intent while a runtime or route is missing or divergent. | Authorities are separated and status observes Podman rather than trusting SQLite alone; reconcile repairs unambiguous drift explicitly. | Complete convergence and ambiguity handling are v0.4 reconciliation work. |
| Weak deployment attribution | Make a deployment difficult to attribute to a person or repository workflow. | Deployment history records activation attempts and source revisions. | Shared deployment keys and the absence of a complete audit identity limit attribution. |
| Health-check evasion | Promote malicious or semantically broken code that returns the expected status. | Internal and public health checks gate promotion and prevent promoting an unavailable candidate. | HTTP health proves bounded reachability/status, not application correctness or safety. |

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

## Out of Scope Security Properties

Pneuma currently does not provide secret management, image vulnerability
scanning, signature or attestation verification, admission policies, runtime
malware detection, WAF features, application authorization, hostile multi-tenant
isolation, or distributed compromise tolerance. DNS and certificate lifecycle are
operator-managed.

## Related Documents

- [`system-context.md`](system-context.md) states scope and non-goals.
- [`architecture.md`](architecture.md) describes current effects and authorities.
- [`../decisions/0006-restricted-ci-interface.md`](../decisions/0006-restricted-ci-interface.md)
  explains the CI interface rationale.
