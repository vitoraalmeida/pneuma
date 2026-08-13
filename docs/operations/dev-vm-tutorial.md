# Pneuma Development VM (Debian 13)

Tutorial for creating and preparing a Debian 13 VM that reproduces the relevant
properties of the production VPS and serves as the standard target for Pneuma
integration and E2E tests, without using the VPS as a laboratory. The complete
plan is at `~/Downloads/pneuma-development-vm-plan.md`; this document is the
operational walkthrough.

This tutorial is intended for a KVM virtual machine managed by libvirt through
the `qemu:///system` connection. Its `virsh` commands, NAT networking, and
snapshot procedures assume that environment.

The VM is Pneuma's **operational host**, not a second development workstation:
editing, compilation, and unit tests remain on the host. The VM validates
rootless Podman, Caddy, Quadlet/systemd, SQLite, permissions, networking,
binary installation, and reboot/recovery.

## 1. Prerequisites

- A **basic** Debian 13 (trixie) VM already created and reachable over SSH from
  the development host. Debian 12 (bookworm) provides Podman 4.3.1, which
  **does not include** the Quadlet generator (`podman-user-generator`).
- A **dedicated** SSH key for the VM, generated on the host before provisioning
  (section 2).
- Small, deterministic fixture applications for testing (section 6).
- A local registry (the `registry:2` container on port 5000) to deliver fixtures
  by digest (section 6.2).

> **Note:** the VM uses libvirt NAT networking with DHCP; its IP may change
> between snapshot restores. When reconnecting after a restore, check the current
> IP (`virsh -c qemu:///system domifaddr <vm>`) and update `~/.ssh/config`.

> **Note:** an account with root access (such as `root` or a user with `sudo`) is
> used only for provisioning. Pneuma runs under the dedicated `pneuma` user,
> without root access, replicating the VPS model.

## 2. Generate the SSH Key and Configure Access

Generate a dedicated key for the VM on the development host (as specified in
the plan, the `pneuma` user does not use the GitHub key):

```bash
ssh-keygen -t ed25519 -f ~/.ssh/pneuma-dev -N "" -C "pneuma-dev VM key"
```

Copy the public key to a VM provisioning account (root or administrative). With
`ssh-copy-id`, provide the IP assigned to the VM on the network (for example,
`192.168.122.50`):

```bash
ssh-copy-id -i ~/.ssh/pneuma-dev.pub root@192.168.122.50
```

Debian accepts root login by key (`PermitRootLogin prohibit-password`) without
exposing the password. Configure the host's `~/.ssh/config` for predictable access:

```text
Host pneuma-dev
    HostName 192.168.122.50
    User root
    IdentityFile ~/.ssh/pneuma-dev
    IdentitiesOnly yes
```

Also add `192.168.122.50 pneuma-dev` to the host's `/etc/hosts`. Confirm:

```bash
ssh pneuma-dev 'hostname'
```

## 3. Provision the Host

With the provisioning key already installed, transfer the script and shared
library and run it as root on the VM. The layout in `/tmp` must preserve the
repository structure (`provision-host.sh` in `dev-vm/`, library in `lib/`),
because the script derives the library path from its own path:

```bash
scp scripts/dev-vm/provision-host.sh pneuma-dev:/tmp/dev-vm/
scp -r scripts/lib pneuma-dev:/tmp/
ssh pneuma-dev 'sudo bash /tmp/dev-vm/provision-host.sh'
```

The VM and VPS apply **the same host invariants**, implemented once in
`scripts/lib/provision-host.sh` and also used by `scripts/bootstrap-vps.sh`. The
script assumes a basic Debian VM and:

1. installs the runtime set (Podman, `uidmap`, `fuse-overlayfs`, Caddy, Git,
   and `curl`) and, as a VM-only convenience, `sqlite3`;
2. verifies the Quadlet generator (`podman-user-generator` >= 4.4);
3. creates the `pneuma` user with `subuid/subgid` and linger;
4. creates Pneuma persistent directories with VPS permissions;
5. configures the VM Caddyfile with `local_certs`, importing only
   `/etc/caddy/applications/*.caddy`, maps fixture domains in `/etc/hosts`, and
   installs Caddy's local CA in the trust store;
6. writes the canonical environment to `/etc/pneuma/environment` and
   `PNEUMA_*`/rootless variables to `pneuma`'s `~/.profile`;
7. validates `caddy validate` and starts the service;
8. confirms rootless Podman with `podman info`.

Unlike production bootstrap, VM provisioning **does not** clone the repository,
compile or install the binary, install the CI key, or run `pneuma doctor`:
provisioning access already exists, and Pneuma installation is a separate step
(section 4).

## 4. Install the Pneuma Binary on the VM

The VM does not compile or clone the repository. The cycle starts with the
binary compiled on the host, and installation is **separate** from provisioning:
transfer the binary and install it to `/usr/local/bin/pneuma`:

```bash
cargo build --release
scp target/release/pneuma pneuma-dev:/tmp/pneuma-new
ssh pneuma-dev 'sudo install -o root -g root -m 0755 /tmp/pneuma-new /usr/local/bin/pneuma'
```

Validate the binary before installing it and run `pneuma doctor` afterward as
the `pneuma` user, so a broken build never replaces a working runtime:

```bash
ssh pneuma-dev '/usr/local/bin/pneuma version'
ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma doctor"'
```

## 5. Verification

On the VM, open the `pneuma` shell and confirm the environment:

```bash
sudo -iu pneuma
pneuma version
pneuma doctor
pneuma app list
```

The freshly provisioned VM does not yet have registered applications; `pneuma
app list` must return an empty list (or the corresponding message), and `pneuma
doctor` must pass every host check.

The normal development cycle becomes:

```text
edit code
    ↓
cargo build --release
    ↓
scp target/release/pneuma pneuma-dev:/tmp/pneuma-new
    ↓
ssh pneuma-dev 'sudo install ... /usr/local/bin/pneuma'
    ↓
ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd $HOME && pneuma doctor"'
    ↓
Pneuma atualizado e validado na VM
```

## 6. Fixture Applications

Keep fixtures independent from the personal site, small, and deterministic:

| Fixture | Behavior | Use |
|---|---|---|
| `healthy-http` | `/healthz` 200; `/` shows version | Happy path, upgrade, rollback |
| `unhealthy-http` | `/healthz` 500 | Active release preservation |
| `slow-start` | Health 200 after a controlled delay | Verification window |
| `bad-port` | Port differs from the declared one | Runtime/configuration failure |
| `redirect-public` | Simple HTTP behind Caddy | Visibility and proxy |

Each fixture resides in `scripts/dev-vm/fixtures/<name>/` with its `pneuma.toml`
and Containerfile.

### 6.1. Copy and Import

Copy the fixtures to the VM checkout (owner `pneuma:pneuma`) and register them
through **remote Git** (v0.2 removed import by local path): for local fixtures,
create a Git repository accessible from the VM and import it by URL.

```bash
scp -r scripts/dev-vm/fixtures pneuma-dev:/var/lib/pneuma/checkouts/
ssh pneuma-dev 'chown -R pneuma:pneuma /var/lib/pneuma/checkouts/fixtures'
# Inside the VM, make the fixture directory available as a remote Git repository:
ssh pneuma-dev 'su - pneuma -c "
  cd /var/lib/pneuma/checkouts/fixtures/healthy-http &&
  git init -q && git add . && git -c user.email=dev@local -c user.name=dev commit -qm initial && 
  git clone --bare . /var/lib/pneuma/checkouts/healthy-http.git"'
ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app import file:///var/lib/pneuma/checkouts/healthy-http.git --manifest pneuma.toml"'
```

The `deploy-all-fixtures.sh` script automates this process for all fixtures: it
creates one local Git repository per fixture in
`/var/lib/pneuma/repos/<fixture>.git` and imports it through `file://`.

> **Warning:** `app import` uses `ON CONFLICT(name) DO NOTHING`; re-importing
> after changing `pneuma.toml` **does not** update the registered delivery. To
> change the repository/delivery of an already registered fixture, update the database:
>
> ```bash
> runuser -u pneuma -- bash -lc 'cd $HOME && sqlite3 /var/lib/pneuma/database/pneuma.sqlite3 \
>   "UPDATE application_delivery_specs SET image_repository = replace(image_repository, \
>   '\''localhost/'\'', '\''localhost:5000/'\'')"'
> ```

### 6.2. Local Registry and Digest Deployment

Fixtures are built and published to a local registry (the `registry:2` container
on port 5000). **The digest used for deployment is the manifest digest in the
registry, not the local Image ID**. Push rewrites the manifest to OCI. To obtain
the registry digest:

```bash
curl -s -H "Accept: application/vnd.oci.image.manifest.v1+json" \
  http://localhost:5000/v2/<fixture>/manifests/latest -D - -o /dev/null \
  | grep -i docker-content-digest
```

Configure the registry as insecure in `/etc/containers/registries.conf.d/pneuma-dev.conf`
(v2 format; the old `[registries.insecure]` is rejected):

```text
[[registry]]
location = "localhost:5000"
insecure = true
```

Build, publish, and deploy:

```bash
podman build -t localhost:5000/<fixture>:latest /var/lib/pneuma/checkouts/fixtures/<fixture>
podman push --tls-verify=false localhost:5000/<fixture>:latest
pneuma app deploy <fixture> --image localhost:5000/<fixture>@sha256:<digest-do-registry>
```

> **Repository enforcement:** deployment accepts only images whose repository
> (`localhost:5000/<fixture>`) matches `[delivery] image` in `pneuma.toml`. The
> `--image` argument accepts only `<repository>@sha256:<hex>` (a bare digest is
> rejected).

### 6.3. Expected Battery Results

| Fixture | Deployment | Note |
|---|---|---|
| `healthy-http` | Succeeded | Host port allocated; `/` responds with the version |
| `unhealthy-http` | Failed | Health check receives 500 |
| `slow-start` | Failed | Health 503 within the verification window |
| `bad-port` | Failed | Connection refused (divergent port) |
| `redirect-public` | Succeeded | Requires Caddy with `local_certs` (section 7) |

Upgrade and rollback use a new/old digest from the same repository; on each
deployment, the previous runtime is removed and the new one receives a new host port.

### 6.4. Cycle Automation Scripts

The scripts in `scripts/dev-vm/` automate the development cycle against the VM
(all accept optional `[ssh-host]`, defaulting to `pneuma-dev`):

Scripts that change Caddy, state directories, binary installation, or reboot the
VM expect the SSH alias to connect as `root`; they neither require nor install
`sudo`. Runtime commands continue under the `pneuma` user.

| Script | What it does | When to use it |
|---|---|---|
| `sync-binary.sh` | `cargo build --release` + scp + install + `pneuma doctor` | After changing Rust code |
| `rebuild-fixtures.sh` | Copies fixtures, builds + pushes to the local registry, shows digests | After editing fixtures/`server.py` |
| `deploy-all-fixtures.sh` | Creates local Git repos, imports each fixture through `file://`, and deploys by digest | After reset or fixture changes |
| `reset-fixtures.sh` | Stops apps, removes units/containers/Caddy fragments/checkouts, recreates the DB | Return to a clean state |
| `overview.sh` | Shows app, container, unit, Caddy, and registry status at once | Quick debugging |
| `e2e.sh` | Reset → rebuild → public/internal HTTPS → failed candidate → upgrade → rollback → reboot/recovery | Runtime and exposure battery |
| `test-branch-deploy.sh` | Creates a Git repo with `main`/`staging`, tags images with each commit SHA, imports by Git URL, and deploys through `--branch` | Validate the Git → OCI flow (phase G) |
| `test-all.sh` | Orchestrates E2E, Git/OCI, CI dispatcher, HTTPS, reboot, and semantic restore; requires 0 FAIL/0 SKIP | Final disposable regression |

Typical development flow:

```bash
scripts/dev-vm/sync-binary.sh        # after every code change
scripts/dev-vm/overview.sh           # inspecionar o estado
```

Complete reset flow:

```bash
scripts/dev-vm/reset-fixtures.sh
scripts/dev-vm/rebuild-fixtures.sh
scripts/dev-vm/deploy-all-fixtures.sh
```

> **Note:** `e2e.sh` reboots the VM (`sudo reboot`) and waits for it to return;
> do not run it during unpersisted work on the VM. `reset-fixtures.sh` deletes
> the database and checkouts, returning the VM to its post-provisioning state.

### 6.5. Final Disposable Regression

There are three distinct environments. Do not substitute one for another:

| Environment | Objective | Destructive operations |
|---|---|---|
| Bootstrap clone | Prove clean Debian 13 bootstrap and rerun by immutable SHA/tag | Only `pneuma-dev-base-test` |
| E2E clone | Prove registry, local TLS, CI dispatcher, rollback, reboot, and restore | Only `pneuma-dev-base-test` |
| Production VPS | Non-destructive smoke testing of real DNS, TLS, and reachability | Never reset, E2E, or restore |

For final regression, clone `pneuma-dev-base` as `pneuma-dev-base-test`,
provision it, sync the binary, and install only the public key from
`~/.ssh/pneuma-ci-test` with the dispatcher's forced command. Then run:

```bash
bash scripts/dev-vm/test-all.sh root@<ip-da-clone> ~/.ssh/pneuma-ci-test
```

The script resets fixtures, uses the local registry, requires HTTPS with the
local CA, reboots the VM, tests CI key boundaries, and proves semantic restore.
Require `0 FAIL` and `0 SKIP`, then destroy and undefine the clone, including
its storage. Run `scripts/test-bootstrap-vps.sh` on another fresh clone for
bootstrap acceptance; it receives a full SHA or tag and shares no state with the
E2E VM.

## 7. Local DNS and Caddy

VM provisioning configures `local_certs`, maps `redirect-public.pneuma.test` to
`127.0.0.1` in `/etc/hosts`, installs Caddy's local CA in the trust store, and
runs `update-ca-certificates`. E2E requires this fixture's HTTPS redirect and
the subsequent transition to `internal`; local TLS cannot be skipped.

To test additional names without public DNS, add them to the VM's `/etc/hosts`:

```text
192.168.122.50 site.pneuma.test
192.168.122.50 api.pneuma.test
```

Public applications undergo an **external health check over HTTPS**; without a
real domain, the VM uses local certificates. The equivalent configuration is:

```caddy
{
    local_certs
}
```

```bash
# CA raiz local do Caddy (instalada automaticamente pelo provisionamento)
sudo cp /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt \
  /usr/local/share/ca-certificates/caddy-local-root.crt
sudo update-ca-certificates
```

Without this, the external health check of a `public` app fails with a TLS error
and deployment is marked Failed (this is unnecessary in production: Let's
Encrypt issues the real certificate).

## 8. Snapshots and Reset

Create at least two snapshots through `virt-manager` or `virsh`
(`-c qemu:///system`):

| Snapshot | State |
|---|---|
| `pneuma-dev-base` | Podman/Caddy/user/directories ready, Pneuma installed |
| `pneuma-dev-fixtures-ready` | Fixtures registered, local registry, Caddy `local_certs`, E2E baseline |

Destructive tests (rollback, reboot, recovery, broken Caddy, inconsistent
database) must begin from `pneuma-dev-base`, without accumulating invisible
state between runs.

> **Note:** the VM uses libvirt DHCP; after restoring a snapshot, its IP may
> change (the current one is in `~/.ssh/config`). Do not trust the old IP.

## 9. Environment Security

- Use a dedicated SSH key for the VM.
- Do not copy production secrets to the VM.
- Use a public registry for fixtures or dedicated read-only credentials.
- Do not expose VM SSH to the Internet (NAT/libvirt network).
- Run Pneuma as the non-root `pneuma` user.
- Restrict root to provisioning and binary installation.
- Disable password login for the `pneuma` user (`passwd -l`).
- The CI key uses `restrict` and a forced command; E2E requires only `version`
  and `deploy healthy-http staging`, and rejects shell, PTY, forwarding,
  agent/X11 forwarding, file reading, and branch injection.

## 10. Next Steps

The `scripts/dev-vm/e2e.sh` battery already covers the main cycle (import,
digest deployment, upgrade, rollback, and reboot). Upgrade/rollback and reboot
were validated on the VM: Quadlet (through `[Install] WantedBy=default.target`)
restores applications at boot with linger enabled, without explicit `systemctl
enable`. With v0.2.1 as the current release, the Git → OCI flow is covered by
`test-branch-deploy.sh` (Git repo with `main`/`staging`, import by `file://` URL,
and deployment through `--branch`) and `e2e.sh` imports fixtures through local
Git repositories. The VPS is used only for final public-integration smoke tests
(real DNS and TLS).

## References

- `scripts/dev-vm/provision-host.sh` — host provisioning.
- `scripts/dev-vm/smoke.sh` — basic verification (version, doctor, app list).
- `scripts/dev-vm/sync-binary.sh` — build + deployment of the binary to the VM.
- `scripts/dev-vm/{rebuild,deploy-all,reset,overview,e2e}.sh` — fixture-cycle
  automation (section 6.4).
- `scripts/dev-vm/fixtures/` — five deterministic fixtures for E2E scenarios
  (section 6).
