# Changelog

## v0.2.0 — Git-aware OCI Delivery (2026-08-11)

Pneuma passa de "opera uma imagem OCI que recebo" para "encontra o artifact do
commit de uma branch e o implanta". O build local foi removido: **CI produz
artifacts, Pneuma descobre e opera artifacts.**

### Added

- `pneuma app import <git-url> --manifest <path>`: importa aplicações a partir
  de repositórios Git remotos, com checkout temporário e persistência do
  `repository_url`/`manifest_path`.
- `pneuma app deploy <app> --branch <branch>`: resolve o commit da branch via
  `git ls-remote`, descobre a imagem OCI pela convenção `image:<commit>` e
  implanta o artifact imutável (mutuamente exclusivo com `--image`).
- `pneuma ci` (dispatcher SSH restrito): aceita apenas `deploy <app> <branch>`
  e `version` via `SSH_ORIGINAL_COMMAND`, com validação anti-injeção.
- Manifest schema v3 sem `[source]`/`[build]`; convenção
  `deploy/<environment>/pneuma.toml`.
- SQLite stores orientados a capacidades (`SqliteApplicationStore`,
  `SqliteDeploymentStore`, `SqliteRuntimeStore`, `SqliteReleaseStore`).
- Migration 0013 para `application_sources` v3.
- Bootstrap VPS com pre-flight checks (Debian 13, internet/DNS, disco, memória,
  CPU, serviços conflitantes, portas 80/443) e validação do estado final.
- Provisionamento de CI deploy key restrita no bootstrap
  (`--ci-public-key`), com forced command `pneuma ci dispatch`.
- Test battery e2e (`scripts/test-battery.sh`) e teste de bootstrap VPS
  (`scripts/test-bootstrap-vps.sh`).
- Scripts e tutorial de VM de desenvolvimento (`scripts/dev-vm/`,
  `docs/operations/dev-vm-tutorial.md`).

### Changed

- Removido build local: `app deploy-source`, `local_build`, `[build]` e
  consume de checkout permanente.
- CLI migrado para `clap` derive.
- Ambiente do Pneuma desacoplado do login shell (`/etc/pneuma/environment`).
- Deploy refatorado: candidate startup, public activation, runtime cleanup e
  progress reporting extraídos; ciclo de vida de recursos candidatos modelado
  explicitamente.
- `XDG_RUNTIME_DIR`/`DBUS_SESSION_BUS_ADDRESS` derivados do uid efetivo.

### Fixed

- `app status`/`stop`/`start` falhavam depois da remoção do container.
- `visibility internal` falhava com `NULL` no domínio.
- Limpeza de units systemd em falha de candidato.
- Vários ajustes de bootstrap (ownership dos diretórios, cwd do doctor,
  warnings do Podman, ordem das operações).

## v0.1.0 — Fundação OCI (2026-08-08)

OCI-first deployments: immutable image pulls, rootless Quadlet runtime, health
checks, Caddy exposure, rollback, and VPS operations.