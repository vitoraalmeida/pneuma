# Pneuma Development VM (Debian 13)

Tutorial for creating and preparing a Debian 13 VM that reproduces the relevant
properties of the production VPS and serves as the standard target for Pneuma
integration and E2E tests, without using the VPS as a laboratory. Every script
mentioned here lives in this repository under `scripts/`.

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

The `scripts/dev-vm/` test scripts do not require this `~/.ssh/config` entry.
They route every remote call through `scripts/lib/remote.sh`, which keeps plain
alias behavior by default and can instead target a host reached through a
forwarded SSH port (for example a raw-QEMU guest) through explicit settings:

```bash
export PNEUMA_SSH_HOST=127.0.0.1
export PNEUMA_SSH_PORT=2222
export PNEUMA_SSH_IDENTITY=/tmp/pneuma-vm/root-key
export PNEUMA_SSH_KNOWN_HOSTS_FILE=/tmp/pneuma-vm/known_hosts
export PNEUMA_SSH_STRICT_HOST_KEY_CHECKING=accept-new
```

With these settings, `scripts/dev-vm/smoke.sh` and the other dev-vm scripts
connect to the forwarded loopback endpoint with the dedicated identity and
per-run known-hosts file instead of an alias; nothing is written to the
developer's `~/.ssh/config` or global `known_hosts`. A positional ssh-host
argument (such as `root@192.168.122.50`) still overrides `PNEUMA_SSH_HOST`,
and the restricted CI-dispatch phase keeps using its explicitly supplied key.

## 3. Provision the Host

With the provisioning key already installed, transfer the script and shared
library and run it as root on the VM. The layout in `/tmp` must preserve the
repository structure (`provision-host.sh` in `dev-vm/`, library in `lib/`),
because the script derives the library path from its own path:

```bash
scp scripts/dev-vm/provision-host.sh pneuma-dev:/tmp/dev-vm/
scp -r scripts/lib pneuma-dev:/tmp/
ssh pneuma-dev 'bash /tmp/dev-vm/provision-host.sh'
```

The VM and VPS apply **the same host invariants**, implemented once in
`scripts/lib/provision-host.sh` and also used by `scripts/bootstrap-vps.sh`. The
script assumes a basic Debian VM and:

1. installs the runtime set (Podman, `uidmap`, `fuse-overlayfs`, Caddy, Git,
   and `curl`) and, as a VM-only convenience, `sqlite3`;
2. verifies the Quadlet generator (`podman-user-generator` >= 4.4);
3. creates the `pneuma` user with `subuid/subgid` and linger;
4. creates Pneuma persistent directories with VPS permissions;
5. configures the VM Caddyfile with `local_certs`, a generic unmatched-host HTTP
   404 fallback, and `/etc/caddy/applications/*.caddy` fragments; maps fixture
   domains in `/etc/hosts`; and installs Caddy's local CA in the trust store;
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
ssh pneuma-dev 'install -o root -g root -m 0755 /tmp/pneuma-new /usr/local/bin/pneuma'
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
runuser -u pneuma -- bash -l
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
ssh pneuma-dev 'install ... /usr/local/bin/pneuma'
    ↓
ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd $HOME && pneuma doctor"'
    ↓
Pneuma updated and validated on the VM
```

## 6. Fixture Applications

Keep fixtures self-contained, small, and deterministic:

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
pneuma app deploy <fixture> --image localhost:5000/<fixture>@sha256:<registry-digest>
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
| `reconciliation-e2e.sh` | Runs the approved drift catalog (runtime, exposure, interrupted deployments, concurrency) against a disposable clone | Reconciliation regression |

Typical development flow:

```bash
scripts/dev-vm/sync-binary.sh        # after every code change
scripts/dev-vm/overview.sh           # inspect the state
```

Complete reset flow:

```bash
scripts/dev-vm/reset-fixtures.sh
scripts/dev-vm/rebuild-fixtures.sh
scripts/dev-vm/deploy-all-fixtures.sh
```

> **Note:** `e2e.sh` reboots the VM (`reboot`) and waits for it to return;
> do not run it during unpersisted work on the VM. `reset-fixtures.sh` deletes
> the database and checkouts, returning the VM to its post-provisioning state.

### 6.5. Final Disposable Regression

There are three distinct environments. Do not substitute one for another:

| Environment | Objective | Destructive operations |
|---|---|---|
| Raw-QEMU E2E | Prove registry, local TLS, CI dispatcher, rollback, reboot, restore, and reconciliation on a fresh Debian 13 guest | Only the qcow2 overlay under `.tmp/pneuma-vm/instance/` |
| Legacy libvirt VM | Manual fixture-cycle development only | Not a required regression path |
| Production VPS | Non-destructive smoke testing of real DNS, TLS, and reachability | Never reset, E2E, or restore |

`scripts/vm/run-e2e.sh` is the standard path for final disposable regression.
It verifies the Debian 13 genericcloud base image, creates a fresh raw-QEMU
qcow2 overlay and cloud-init seed, provisions the guest, builds and syncs the
binary, creates a restricted CI key inside the instance directory, runs the
battery, and destroys the instance on exit whether it passes or fails.

```bash
PNEUMA_VM_RECONCILIATION=1 scripts/vm/run-e2e.sh
```

`PNEUMA_VM_RECONCILIATION=1` appends the full drift catalog after the normal
`test-all.sh` battery. Set `PNEUMA_VM_KEEP=1` only to preserve an instance for
debugging; the next normal run removes any prior instance to preserve the
fresh-guest guarantee.

Prerequisites are the outer-host QEMU/cloud-init tools, `cargo`, `git`, and
SSH/SCP. The harness needs no libvirt template, DHCP lease, host SSH key, or
pre-existing CI key. Per-instance root and CI keys stay under
`.tmp/pneuma-vm/instance/` and are destroyed with the overlay.

`scripts/dev-vm/test-regression.sh` remains available for legacy development
work, but must not be used for disposable end-to-end validation. The VPS remains
reserved for non-destructive smoke tests of real DNS and TLS.

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
# Caddy local root CA (installed automatically by provisioning)
cp /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt \
  /usr/local/share/ca-certificates/caddy-local-root.crt
update-ca-certificates
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

## 9. Next Steps

The `scripts/dev-vm/e2e.sh` battery already covers the main cycle (import,
digest deployment, upgrade, rollback, and reboot). Upgrade/rollback and reboot
were validated on the VM: Quadlet (through `[Install] WantedBy=default.target`)
restores applications at boot with linger enabled, without explicit `systemctl
enable`. With v0.5.4 as the current release, the Git → OCI flow is covered by
`test-branch-deploy.sh` (Git repo with `main`/`staging`, import by `file://` URL,
and deployment through `--branch`) and `e2e.sh` imports fixtures through local
Git repositories; `reconciliation-e2e.sh` covers the drift catalog. The VPS is
used only for final public-integration smoke tests (real DNS and TLS).

## 10. Portable Raw-QEMU Disposable VM (`scripts/vm/`)

Sections 1–9 describe legacy persistent libvirt development tooling. The
`scripts/vm/` harness is the standard fully disposable path to the same Debian
13 guest model:
a raw `qemu-system-x86_64` launcher that needs no persistent daemon and runs
identically on a developer Linux host and a GitHub Actions runner
(`.github/workflows/e2e.yml`). The guest is the integration target; the outer
host only needs VM/SSH tooling and Rust (`qemu-system-x86`, `qemu-utils`,
`cloud-image-utils`, `git`, `cargo`, `ssh`/`scp`).

Key properties:

- The immutable Debian 13 genericcloud base image is downloaded once into
  `.tmp/pneuma-vm/base/` and verified against the published `SHA512SUMS` on
  every use (a stale cache is re-downloaded); each run writes a fresh qcow2
  overlay in `.tmp/pneuma-vm/instance/`. `stop` preserves the instance; only
  `destroy` removes it.
- Acceleration is auto-detected: KVM when `/dev/kvm` is usable, TCG otherwise
  (`PNEUMA_VM_ACCEL=kvm|tcg|auto`); the choice is printed on start.
- SSH forwarding binds to `127.0.0.1` only, with a per-instance ephemeral key
  and a disposable known-hosts file; nothing touches `~/.ssh/config`.

One-command paths, all ephemeral by default (any previous instance is
destroyed first, and the instance is destroyed again on exit):

```bash
scripts/vm/smoke.sh     # start -> provision -> sync-binary -> dev-vm smoke
scripts/vm/run-e2e.sh   # smoke path + ephemeral restricted CI key + full test-all battery
```

`run-e2e.sh` is the full regression: the `test-all.sh` battery (failed-candidate
preservation, digest deployment, upgrade, rollback, the real guest reboot with
post-reboot recovery, branch deployment, restricted CI dispatch, backup/restore,
smoke). On failure it prints state, serial-console, and test-log diagnostics
before destroying the instance and keeps the original exit code.
`PNEUMA_VM_KEEP=1` preserves the instance for debugging instead (it then
refuses to run over a pre-existing instance); unset it and rerun when done.

The same entry point runs in CI:

- **Manual:** the `E2E` workflow (`.github/workflows/e2e.yml`) via
  `workflow_dispatch` on a standard hosted runner.
- **Automatic:** the same workflow runs for pull requests targeting `main` and
  every push to `main`; non-main pushes run the smaller raw-QEMU smoke path.

Both modes require no GitHub secrets: every SSH key is generated inside the
run and dies with the instance, and failure artifacts contain only text logs
(never the instance directory with its keys or the mutable disk).

## References

- `scripts/dev-vm/provision-host.sh` — host provisioning (with `scripts/lib/provision-host.sh`).
- `scripts/dev-vm/smoke.sh` — basic verification (version, doctor, app list).
- `scripts/dev-vm/sync-binary.sh` — build + deployment of the binary to the VM.
- `scripts/dev-vm/{rebuild,deploy-all,reset,overview,e2e,test-branch-deploy}.sh` —
  fixture-cycle automation (section 6.4).
- `scripts/dev-vm/test-all.sh` — full disposable regression battery (section 6.5).
- `scripts/dev-vm/reconciliation-e2e.sh` — drift-catalog regression (section 6.5).
- `scripts/dev-vm/test-regression.sh` — legacy libvirt disposable-lifecycle
  orchestrator; not a final-regression requirement.
- `scripts/bootstrap-vps.sh`, `scripts/test-bootstrap-vps.sh` — production
  bootstrap and its acceptance test.
- `scripts/dev-vm/fixtures/` — five deterministic fixtures for E2E scenarios
- `scripts/vm/start-debian13.sh` (with `wait-for-ssh.sh`, `stop.sh`,
  `destroy.sh`, `diagnostics.sh`, `instance.sh`) — raw-QEMU Debian 13
  lifecycle (section 10).
- `scripts/vm/provision.sh`, `scripts/vm/smoke.sh`, `scripts/vm/run-e2e.sh` —
  one-command disposable provisioning, smoke, and full E2E (section 10).
- `.github/workflows/e2e.yml` — the same disposable E2E on GitHub Actions,
  manual and on `main` (section 10).
  (section 6).
