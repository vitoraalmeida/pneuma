# Complete Pneuma Host Setup

Step-by-step guide to turn a Debian 13 VPS into a production Pneuma host and
connect an application repository to it through GitHub Actions. Covers key
generation, running `bootstrap-vps.sh`, import and deployment, and deployment
workflow configuration.

`scripts/bootstrap-vps.sh` installs everything (packages, the `pneuma` user,
rootless Podman, Caddy, binary) **and** prepares the CI identity. After it
completes successfully, the host is ready to import and deploy applications.

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
  --ref v0.3.0
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
leaves those host settings unchanged. Run the following as `root` on the VPS,
replacing `v0.3.0` with the target immutable tag:

```bash
# Back up the catalog before an update that may apply a database migration.
/usr/local/bin/pneuma database backup /var/backups/pneuma-before-v0.3.0.sqlite3

PNEUMA_HOME=/home/pneuma
PNEUMA_SOURCE_PATH="$PNEUMA_HOME/pneuma"
PNEUMA_VERSION=v0.3.0

runuser -u pneuma -- env HOME="$PNEUMA_HOME" \
  git -C "$PNEUMA_SOURCE_PATH" fetch --prune --tags origin
TARGET_SHA="$(runuser -u pneuma -- env HOME="$PNEUMA_HOME" \
  git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify "refs/tags/$PNEUMA_VERSION^{commit}")"
runuser -u pneuma -- env HOME="$PNEUMA_HOME" \
  git -C "$PNEUMA_SOURCE_PATH" checkout --force --detach "$TARGET_SHA"
runuser -u pneuma -- bash -lc "source '$PNEUMA_HOME/.cargo/env' && cd '$PNEUMA_SOURCE_PATH' && cargo build --release"
install -o root -g root -m 0755 \
  "$PNEUMA_SOURCE_PATH/target/release/pneuma" /usr/local/bin/pneuma

/usr/local/bin/pneuma version
runuser -u pneuma -- bash -lc 'cd "$HOME" && pneuma doctor'
```

The first command that opens the database, such as `pneuma doctor`, applies
pending forward-only migrations. Existing deployed applications continue to run
while the binary is replaced because Quadlet supervises their containers.

Before upgrading across a version with a migration, validate the target version
on a disposable VM and keep the backup until verification succeeds. v0.3 rejects
new local-path `pneuma app import` calls; existing imported applications remain
registered. Downgrading after a migration is unsupported: restore the pre-update
database backup before running an earlier Pneuma binary.

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
ssh-keyscan 46.202.150.155   # or the host hostname/domain
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
- The full workflow (image build, smoke test, push to GHCR, deployment through
  staging/main) can be copied from the repository
  `github.com/vitoraalmeida/vitoralmeida.tech`, file
  `.github/workflows/deploy.yml`.

## 6. Final Check

```bash
# On the host
sudo -iu pneuma
pneuma doctor          # ok

# On your machine
ssh -i ~/.ssh/pneuma-ci pneuma@<host> "version"   # responds with the version

# After a push to the workflow branch
# → the workflow publishes the image and requests deployment
# On the VPS, `pneuma app status <app>` should report Running and
# `pneuma app list` should show the application as Deployed
```

### 6.1. Production Smoke Test

On the VPS, run only non-destructive smoke tests: `pneuma doctor`, `pneuma app
list`, `pneuma app status <app>`, and an HTTPS request to the public domain. Do
not run `reset-fixtures.sh`, `e2e.sh`, `test-all.sh`, database restore, or
bootstrap tests on the VPS. Clean bootstrap/rerun and E2E regression belong on
disposable Debian 13 clones, as described in the
[`VM tutorial`](operations/dev-vm-tutorial.md).

The cycle is complete: Git push → CI builds/publishes → `deploy <app> <branch>`
→ Pneuma resolves, validates, and promotes; Caddy exposes the public application
automatically.
