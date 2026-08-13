# Pneuma Security Model

**Status:** living document - describes current trust boundaries and controls,
not a claim that every deployment is secure against every threat.

## Assets

- Ability to deploy Applications.
- Application repository identity and imported manifest configuration.
- OCI artifact identity.
- Pneuma SQLite intent and history.
- Managed Caddy routes.
- Active RuntimeInstance identity and host loopback endpoints.
- CI deployment credentials and host filesystem.

## Actors and Trust

| Actor | Trust relationship |
|---|---|
| Operator | Trusted host administrator who configures the host, repositories, DNS, and credentials. |
| Application repository | Trusted source of import-time configuration and requested Git revision. |
| CI workflow | Trusted to build/publish the expected artifact and request deployment through its restricted key. |
| OCI registry | Trusted for the availability and bytes addressed by a resolved digest. |
| Application container | Less trusted than the host; not treated as a hostile-tenant boundary. |
| Internet client | Untrusted public client. |

## Trust Boundaries

```text
CI deploy private key
    |
    | SSH
    v
------------------------------ host boundary ------------------------------
restricted authorized_keys command -> pneuma ci dispatch -> Pneuma CLI
                                                  |             |
                                                  |             +-- SQLite
                                                  |
                                                  +-- rootless Podman / systemd
                                                  |          |
                                                  |          +-- application container
                                                  |
                                                  +-- Caddy
                                                               |
------------------------------ public boundary ----------------------------
                                                               |
                                                            Internet
```

Git and the OCI registry are external dependencies. SQLite, authorized keys,
systemd configuration, Caddy configuration, and the local host administrator are
trusted local boundaries. Root compromise defeats Pneuma's local controls.

## Controls

| Mechanism | Threat addressed | Limit |
|---|---|---|
| Digest-pinned Release | Mutable tag changing after selection | Does not verify image signature or publisher provenance. |
| Commit-SHA tag resolution | Selecting the artifact CI associated with a resolved revision | Assumes CI published `repository:<commit-sha>` correctly. |
| Manifest repository allow-list | Deploy command selecting an unrelated repository | Does not authorize a particular branch per application. |
| No fallback to `latest` | Silent substitution of an unavailable requested artifact | Registry availability remains external. |
| Rootless Podman | Broadens the privilege boundary less than rootful runtime | Not hostile-workload or kernel-exploit isolation. |
| Loopback runtime binding | Direct public access to application runtime ports | Host-local users and Caddy remain able to reach loopback. |
| Caddy managed routes | Explicit ingress materialization and external health boundary | DNS, certificates, and Caddy host configuration remain operator-managed. |
| Candidate health verification | Promoting an unhealthy replacement | Does not guarantee later application correctness. |
| Forced SSH dispatcher | CI key becoming an arbitrary shell | Key can deploy any imported Application and syntactically valid branch/tag. |

## Compromise Scenarios

| Compromise | Expected boundary |
|---|---|
| CI deployment key stolen | Restricted to dispatcher `version` and deploy requests; no interactive shell, but no per-key app/branch policy exists. |
| Application container compromised | Rootless runtime and loopback reduce host/public exposure; Pneuma does not claim hostile-tenant isolation. |
| Registry tag changed | Deploy resolves the selected full commit-SHA tag once and persists the resulting digest; later Release use is digest-pinned. |
| Registry digest bytes unavailable | Deployment or runtime recovery can fail; availability is external. |
| SQLite modified | Desired intent, history, and logical identity are compromised. |
| Caddy configuration modified | Public routing and TLS/ingress behavior are compromised. |
| Host root compromised | Pneuma cannot protect host state or workloads. |

## Out of Scope Security Properties

Pneuma currently does not provide secret management, image vulnerability
scanning, signature or attestation verification, admission policies, runtime
malware detection, WAF features, application authorization, hostile multi-tenant
isolation, or distributed compromise tolerance. DNS and certificate lifecycle are
operator-managed.

## Related Documents

- [`system-context.md`](system-context.md) states scope and non-goals.
- [`architecture.md`](architecture.md) describes current effects and authorities.
- [`../decisions/0007-restricted-ssh-ci-interface.md`](../decisions/0007-restricted-ssh-ci-interface.md)
  explains the CI interface rationale.
