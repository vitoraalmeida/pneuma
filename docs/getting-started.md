# Complete Pneuma Host Setup

Step-by-step guide to turn a Debian 13 VPS into a production Pneuma host and
connect an application repository to it through GitHub Actions. Covers key
generation, running `bootstrap-vps.sh`, import and deployment, and deployment
workflow configuration.

For why Pneuma uses this model, see
[`architecture/system-context.md`](architecture/system-context.md). For current
trust boundaries, including the restricted CI key, see
[`architecture/security-model.md`](architecture/security-model.md).

`scripts/bootstrap-vps.sh` installs everything (packages, the `pneuma` user,
rootless Podman, Caddy, binary) **and** prepares the CI identity. After it
completes successfully, the host is ready to import and deploy applications.

## Host Provisioning Flow

```mermaid
flowchart LR
    operator[Root operator]
    bootstrap[bootstrap-vps.sh]
    preflight[Host preflight]
    packages[Runtime and compiler packages]
    account[pneuma account, subids, and linger]
    directories[Persistent directories and permissions]
    source[Pinned source checkout]
    binary[Build and install Pneuma binary]
    environment[Canonical host environment]
    ci_key[Optional restricted CI public key]
    caddy[Caddy baseline]
    runtime[User manager and rootless Podman]
    doctor[pneuma doctor and final-state checks]

    operator -->|1. run as root with source and optional CI key| bootstrap
    bootstrap -->|2. verify Debian, network, resources, and ports| preflight
    preflight -->|3. install dependencies| packages
    packages -->|4. create and validate deployment identity| account
    account -->|5. create host paths| directories
    directories -->|6. clone and resolve tag or full commit SHA| source
    source -->|7. compile and install root-owned binary| binary
    binary -->|8. write environment| environment
    environment -.->|9. when supplied, install forced-command key| ci_key
    environment -->|10. validate and apply ingress baseline| caddy
    ci_key -->|10. continue provisioning| caddy
    caddy -->|11. start and verify rootless runtime| runtime
    runtime -->|12. diagnose and verify final state| doctor
```

The CI-key step is optional. When the Pneuma source repository requires a newly
generated SSH deploy key, bootstrap stops after printing the public key and the
operator reruns it after granting repository access.

## 1. Prerequisites

- Debian 13 (trixie) VPS with root access over SSH. Debian 12 **is not** suitable
  (Podman 4.3.1 lacks the Quadlet generator required by Pneuma).
- Internet access, resolving DNS, free ports 80/443, and at least 2 GiB of RAM
  and 3 GiB of free disk space (the script checks everything before changing the
  system and aborts with actionable messages if anything is missing).
- The applications' public domain (and `*.staging`, if using pre-production)
  pointing through DNS A/AAAA records to the VPS IP.
- No nginx/apache running on the VPS: they are blocking conflicts.

## 2. Generate the Keys

The Pneuma host uses two different identities. They serve distinct roles; do
not confuse them:

### 2.1. `pneuma` User Key (host access to Git repositories)

Used by Pneuma itself for `git clone`/`git fetch` during branch deployments;
needed only if the **application** repository (or Pneuma itself) is private over
SSH. **Bootstrap generates this key automatically** when you provide an SSH
source URL:

```bash
bash scripts/bootstrap-vps.sh git@github.com:USER/pneuma.git
```

On the first run, the script creates `~pneuma/.ssh/id_ed25519`, prints the
public key, and stops. Add that public key as a read-only *deploy key* to the
private repository or repositories, then rerun the script to continue.

> If the Pneuma source URL is public over HTTPS (the usual URL for cloning
> Pneuma itself, which is not private), this key is never created. In that case,
> for applications with **private** Git repositories, generate a key for the
> `pneuma` user manually on the VPS or configure access another way; bootstrap
> only supports the public-HTTPS/private-SSH flow for the Pneuma source.

### 2.2. CI Key (deployment through GitHub Actions)

Used on GitHub Actions machines to authenticate to the host as `pneuma` and
trigger deployments. As bootstrap itself instructs: **generate the pair on a
trusted machine, never on the VPS**. The private key becomes a GitHub secret,
and only the public key is provided to the script:

```bash
ssh-keygen -t ed25519 -f ~/.ssh/pneuma-ci -N "" -C "pneuma-ci deploy key"
```

This creates `~/.ssh/pneuma-ci` (private) and `~/.ssh/pneuma-ci.pub` (public).
The script accepts `ssh-ed25519`, `ssh-rsa`, and `ecdsa-sha2-nistp*`.

## 3. Run Bootstrap

With all keys ready, run as root on the VPS, providing the **public** CI key
with `--ci-public-key <path>`:

```bash
bash bootstrap-vps.sh \
  git@github.com:USER/pneuma.git \
  --ci-public-key ~/.ssh/pneuma-ci.pub
```

The script installs the CI public key in `~pneuma/.ssh/authorized_keys` with
`restrict,command="/usr/local/bin/pneuma ci dispatch"`; in other words, anyone
authenticating with this key can **only** run the restricted dispatcher (no
shell). At the end, in addition to `pneuma doctor`, it prints the same
`DEPLOY_SSH_KEY` notice and test command.

There are two ways to transfer the file to the VPS:

```bash
# A) copy with scp and read it from the host
scp ~/.ssh/pneuma-ci.pub root@<ip>:pneuma-ci.pub
ssh root@<ip> 'bash scripts/bootstrap-vps.sh git@github.com:USER/pneuma.git --ci-public-key pneuma-ci.pub'
# B) pipe the content directly (without a temporary file)
ssh root@<ip> 'bash bootstrap-vps.sh git@github.com:USER/pneuma.git --ci-public-key /dev/stdin' < ~/.ssh/pneuma-ci.pub
```

The host invariants applied by bootstrap (runtime packages, the `pneuma` user
and group, subids, linger, directories, canonical environment, Caddy, and
rootless Podman) reside in a shared library, `scripts/lib/provision-host.sh`,
which is also used by development VM provisioning. The library does not change
bootstrap-specific behavior: cloning the source, compiling and installing the
binary, and installing the CI key remain in `bootstrap-vps.sh`.

### 3.1. Pin the Pneuma Version with `--ref`

For reproducible installations, bootstrap accepts `--ref` with **only** a full
commit SHA (`[0-9a-f]{40}`) or existing Git tag; branches and abbreviated SHAs
are rejected before any host change:

```bash
bash bootstrap-vps.sh \
  git@github.com:USER/pneuma.git \
  --ci-public-key ~/.ssh/pneuma-ci.pub \
  --ref v0.5.4
```

Each run (including reruns) resolves `--ref`, performs a **forced** detached
checkout of the resolved commit, and compiles exactly that commit; APT, user,
subid, and Caddy configuration remain idempotent. On a rerun, the active managed
Caddy itself may occupy ports 80/443; any other process on those ports remains a
blocker. The Caddy baseline is generated as a candidate in the same directory,
validated before the atomic replacement, and backed up only when its contents
change; an unchanged rerun creates no backup and does not reload the service.
Without `--ref`, the script compiles the repository's default branch, as before.

### 3.2. Update the Pneuma Binary

For a routine version update, do not rerun bootstrap. Bootstrap converges the
entire host (packages, account, Caddy, and environment); a binary-only update
leaves those host settings unchanged. Run the updater as `root` on the VPS,
replacing `v0.5.4` with the target immutable tag:

```bash
bash /home/pneuma/pneuma/scripts/update-pneuma.sh --ref v0.5.4
```

For the first update from a release that predates this script, copy the updater
from the development machine and run the copied file as `root`:

```bash
scp scripts/update-pneuma.sh root@<host>:/tmp/update-pneuma.sh
ssh root@<host> 'bash /tmp/update-pneuma.sh --ref v0.5.4'
```

The updater requires a tag or full commit SHA, rejects branches and abbreviated
SHAs, creates a database backup in `/var/backups/pneuma/`, fetches and checks out
the target commit, builds and installs the binary, then verifies its version and
runs `pneuma doctor`. Existing deployed applications continue to run while
the binary is replaced because Quadlet supervises their containers.

The database schema is versioned by a single current baseline; Pneuma does not
upgrade existing databases across incompatible schema changes. A database
created by an older incompatible schema is rejected at open time with an
explicit error; restore the matching backup or start from a fresh database.

When an update changes the bootstrap-managed Caddy baseline, rerun bootstrap
after the binary updater completes to apply the candidate configuration
atomically:

```bash
bash /home/pneuma/pneuma/scripts/bootstrap-vps.sh \
  <pneuma-source-url> \
  --ci-public-key <existing-ci-public-key> \
  --ref <target-tag>
```

For the generic unmatched-host fallback, this makes HTTP requests to an
internalized Application's former hostname return `404 Not Found`. HTTPS returns
404 only when TLS can complete; otherwise a TLS handshake failure is expected.
Remove public DNS separately when the hostname should no longer resolve.

### 3.3. Disposable Acceptance

Before changing a VPS, validate bootstrap and rerun on a disposable Debian 13
clone using a full SHA or immutable tag. This acceptance uses
`scripts/test-bootstrap-vps.sh`; it may install packages, create the `pneuma`
user, and test the CI key only on the clone. The production VPS is limited to
non-destructive smoke testing of DNS, TLS, and reachability.

### 3.4. Confirmation

As `pneuma` (direct login with the provisioning key or `sudo -iu pneuma`):

```bash
pneuma doctor        # all host checks pass
pneuma app list      # empty (no applications yet)
```

From your machine, confirm the CI identity by authenticating with the **private**
key: it should respond to `version`:

```bash
ssh -i ~/.ssh/pneuma-ci pneuma@<host-ip> "version"
```

If it responded, the host is ready to be managed by GitHub Actions.

## 4. Import and Deploy Manually

The standard Pneuma flow with CI is: CI builds and publishes the image (tagged
with the commit SHA), then requests deployment from the host. You can import
and deploy manually from the host at any time.

### 4.1. Import the Application

Log in as `pneuma` and import from the Git repository, specifying the delivery
manifest with `--manifest`:

```bash
sudo -iu pneuma
pneuma app import https://github.com/owner/my-app --manifest deploy/staging/pneuma.toml
```

Pneuma clones the repository **only temporarily** (reads `pneuma.toml`, persists
the application, and removes the checkout) and registers the application with
the declared delivery (OCI image, port, health check, visibility). `--manifest`
is the path to `pneuma.toml` **inside the repository**. `app import` accepts
only Git URLs; local paths are rejected and `file://` is reserved for local test
repositories.

### 4.2. Branch Deployment (recommended with CI)

CI has already published the image tagged with the SHA; Pneuma resolves branch →
SHA → tag → digest:

```bash
pneuma app deploy my-app --branch staging
```

### 4.3. Digest Deployment (immutable, manual)

```bash
pneuma app deploy my-app --image ghcr.io/owner/my-app@sha256:<digest>
```

Deployment validates the image (pull + health check) before promotion; if it
fails, the previous version remains active. Monitor it with:

```bash
pneuma app status my-app
pneuma app deployments my-app
```

## 5. Configure GitHub Actions

The application repository workflow needs repository- or account-level secrets
and variables. The required secrets are `DEPLOY_SSH_KEY` and
`DEPLOY_KNOWN_HOSTS`; the variables are `DEPLOY_HOST` and `DEPLOY_USER`.

### 5.1. Secrets

In GitHub (profile → Settings → Secrets and variables → Actions → New repository
secret), create:

- **`DEPLOY_SSH_KEY`** — contents of the CI private key, the
  `~/.ssh/pneuma-ci` file. Paste the entire file text, including the
  `-----BEGIN OPENSSH PRIVATE KEY-----`/`-----END...-----` block, with the last
  line ending in a newline. Errors here are the #1 cause of
  `Permission denied (publickey)`.
- **`DEPLOY_KNOWN_HOSTS`** — host fingerprint line or lines, obtained with
  `ssh-keyscan`. With the VPS already reachable:

```bash
ssh-keyscan <host>   # VPS hostname or IP address
```

Paste the output as the value (if you also access the IP directly, include the
IP and hostname lines). Do not add anything beyond the
`<host> <algorithm> <key>` lines.

> Tip: if access is unavailable at configuration time, you can generate
> known_hosts locally using `ssh-keyscan` from the same network; the correct
> `ecdsa-sha2-*`/`ssh-ed25519` fingerprint presented on the first `ssh` remains
> the same.

### 5.2. Variables

In the same panel, under "Variables", create:

- **`DEPLOY_HOST`** — VPS IP or hostname.
- **`DEPLOY_USER`** — `pneuma` (not an administrator).

Keeping the IP/host as a variable allows changing VPSs without editing the workflow.

### 5.3. (Optional) Account Scope

Because the key is restricted to the dispatcher, one account-level set of
secrets serves every repository in the account: go to **Account Settings →
Secrets and variables → Actions** and create the same four items once.
Repositories that need different hosts create their own overriding secrets.

### 5.4. Example Workflow

The application repository needs a workflow that (1) builds and publishes the
OCI image and (2) triggers deployment. Once the image is published with SHA and
branch tags, the deployment step is:

```yaml
- name: Deploy to staging
  env:
    DEPLOY_SSH_KEY: ${{ secrets.DEPLOY_SSH_KEY }}
    DEPLOY_KNOWN_HOSTS: ${{ secrets.DEPLOY_KNOWN_HOSTS }}
  run: |
    mkdir -p ~/.ssh
    printf '%s\n' "$DEPLOY_SSH_KEY" > ~/.ssh/deploy_key
    printf '%s\n' "$DEPLOY_KNOWN_HOSTS" > ~/.ssh/known_hosts
    chmod 600 ~/.ssh/deploy_key
    ssh -i ~/.ssh/deploy_key -o BatchMode=yes \
      ${{ vars.DEPLOY_USER }}@${{ vars.DEPLOY_HOST }} \
      "deploy my-app staging"
```

- The SSH command is the dispatcher itself: `deploy <application> <branch>` (the
  `pneuma ci dispatch` subcommand is invoked through a forced command; **do not**
  write `pneuma app deploy ... --branch ...`, as it will be rejected).
- `my-app` must already be imported on the host (section 4.1) **before** the
  first CI deployment.
- The complete workflow must build and push immutable SHA and branch tags before
  it requests deployment through the restricted SSH command.

## 6. Final Check

```bash
# On the VPS, as pneuma
sudo -iu pneuma
pneuma version
pneuma doctor
pneuma app list

# On your machine
ssh -i ~/.ssh/pneuma-ci pneuma@<host> "version"   # responds with the version

# After the first CI deployment, on the VPS as pneuma
pneuma app status <app>
pneuma app deployments <app>
curl -fsS https://<public-domain>/healthz
```

### 6.1. Production Smoke Test

On the VPS, run only non-destructive smoke tests: `pneuma version`, `pneuma
doctor`, `pneuma app list`, `pneuma app status <app>`, `pneuma app deployments
<app>`, and an HTTPS request to the public domain. Do not run
`reset-fixtures.sh`, `e2e.sh`, `test-all.sh`, database restore, or bootstrap
tests on the VPS. Clean bootstrap/rerun acceptance belongs to
`scripts/test-bootstrap-vps.sh`; full functional acceptance belongs to
`scripts/dev-vm/test-all.sh`. Run both only on disposable Debian 13 clones, as
described in the [`VM tutorial`](operations/dev-vm-tutorial.md).

The cycle is complete: Git push → CI builds/publishes → `deploy <app> <branch>`
→ Pneuma resolves, validates, and promotes; Caddy exposes the public application
automatically.

## 7. Reference

### Commands

| Command | Description |
|---|---|
| `pneuma system create <name>` | Create a System grouping. |
| `pneuma system list` | List Systems. |
| `pneuma system show <name>` | Show a System and its Applications. |
| `pneuma app import <git-url> [--manifest <path>]` | Import an Application from a Git repository. |
| `pneuma app list` | List registered Applications. |
| `pneuma app deploy <app> --branch <branch-or-tag>` | Resolve and deploy the artifact for a Git revision. |
| `pneuma app deploy <app> --image <repository@sha256:...>` | Deploy an explicit digest-pinned image. |
| `pneuma app visibility set <app> <public\|internal>` | Set desired public visibility. |
| `pneuma app status <app>` | Show desired and observed runtime state. |
| `pneuma app start <app>` | Start a stopped Application. |
| `pneuma app stop <app>` | Stop a running Application. |
| `pneuma app deployments <app>` | List Deployment history. |
| `pneuma deployment rollback <app>` | Deploy the prior successful Release as a new rollback Deployment. |
| `pneuma database backup <path>` | Create a consistent SQLite backup. |
| `pneuma database restore <path>` | Validate a current-schema SQLite backup and restore it atomically; rejects incompatible backups. |
| `pneuma ci dispatch` | Restricted SSH dispatcher; not for direct interactive use. |
| `pneuma tui` | Open the interactive application catalog; requires interactive stdin and stdout. |
| `pneuma version` | Print version without opening the database. |
| `pneuma doctor` | Verify host prerequisites. |

Place `--verbose` before the command for step-by-step progress.

### Terminal Interface

Run `pneuma tui` from an interactive terminal. The opening catalog lists
registered Applications and whether each has a successful deployment; it does
not claim that an Application is running. Use Up/Down or `j`/`k` to select an
Application, Enter to inspect its persisted details, deployment history, and an
on-demand runtime observation. Use `r` to refresh, Esc to return to the catalog,
and `q` to quit. A failed read stays visible in the interface and does not end
the session.

In an Application detail view, use `s` to start, `x` to stop, `c` to reconcile,
`p` to set public visibility, or `i` to set internal visibility. Each action
opens a confirmation: Enter or `y` executes it; Esc or `n` cancels it. Use `d`
to open the deployment form: Tab switches between the branch and the
digest-pinned image source, the typed value is edited with printable keys and
Backspace, and Enter submits it to the existing deploy command. Use `b` to roll
back to the previous successful release after confirming. While a deployment or
rollback runs, its semantic steps stream into a `Deployment progress` panel and
the final typed result lands in the `Last action` panel. The TUI
shows the existing error class and diagnostic when an action fails, then refreshes
the affected data. Its `Last action` panel shows a completed action only for the
Application being inspected. A new confirmed action replaces queued refresh reads
and runs after the active command, without leaving the detail view.

### Manifest

The manifest convention is `deploy/<environment>/pneuma.toml` in the application
repository. Import receives the repository URL and manifest path; deployment
receives the branch or tag. The manifest does not contain either value.

```toml
schema_version = 3

[system]
name = "my-system"

[application]
name = "my-app"

[delivery]
type = "oci"
image = "ghcr.io/user/my-app"

[runtime]
container_port = 8080
healthcheck_path = "/healthz"
expected_status = 200

[exposure]
default_visibility = "public"
domain = "my-app.example.com"
```

| Field | Meaning |
|---|---|
| `schema_version` | Manifest schema version; currently `3`. |
| `system.name` | System grouping identifier. |
| `application.name` | Application identifier used by commands. |
| `delivery.type` | Delivery model; currently `oci`. |
| `delivery.image` | OCI repository CI publishes to; it is not a digest reference. |
| `container_port` | Container port exposed to the loopback runtime endpoint. |
| `healthcheck_path` | Absolute internal health path. |
| `expected_status` | Required HTTP health status. |
| `default_visibility` | `internal` or `public`. |
| `domain` | Required when default visibility is public. |

`app import` accepts Git URLs; local paths are rejected and `file://` is reserved
for local test repositories. Import is create-only, so changing a registered
manifest requires re-registering under a new name or manual reconfiguration.

### Configuration

| Variable | Default | Description |
|---|---|---|
| `PNEUMA_HOST_ENVIRONMENT_FILE` | `/etc/pneuma/environment` | Host environment file read at startup. |
| `PNEUMA_DATABASE_PATH` | `/var/lib/pneuma/database/pneuma.sqlite3` | SQLite database location. |
| `PNEUMA_WORKSPACE_PATH` | `/var/lib/pneuma/checkouts` | Temporary Git checkout directory. |
| `PNEUMA_CADDY_MANAGED_PATH` | `/etc/caddy/applications` | Managed Caddy fragment directory. |
| `PNEUMA_CADDYFILE_PATH` | `/etc/caddy/Caddyfile` | Main Caddyfile location. |
| `PNEUMA_RUNTIME_PORT_RANGE` | `30000-39999` | Loopback runtime port range. |
| `PNEUMA_QUADLET_DIR` | `$HOME/.config/containers/systemd` | Quadlet unit directory. |

The host environment file is optional; when present it must be readable, valid
UTF-8, and fully valid, otherwise startup fails with a single `error:` line
before any command runs. Blank lines and `#` comments are ignored; every other
line must be `NAME=VALUE` (the first `=` separates, additional `=` characters
and inline `#` belong to the value), values may be empty, and duplicate names
are rejected. Caller-supplied variables override file values. After startup,
either `HOME` or `PNEUMA_QUADLET_DIR` must be set to a nonempty value.
