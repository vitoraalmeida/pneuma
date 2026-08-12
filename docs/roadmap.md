# Roadmap consolidado do Pneuma — v0.1 a v0.7

**Status:** documento vivo — contrato de evolução do projeto
**Aplicação-piloto:** `vitoralmeida.tech`

## Fluxo unificado (v0.1 → v0.2)

```text
Git branch
    ↓ (v0.2: app deploy --branch)
git ls-remote → CommitSha
    ↓ (v0.2: convenção image:<commit>)
OCI Registry (ghcr.io)
    │   image@sha256:...
    ▼
Create Release
    │
    ▼
DeployRelease
```

O CI produz o artifact (como `image:<commit-sha>`), o Pneuma descobre o artifact
do commit e o opera. A v0.1 ainda aceita a Release vinda de build local
(`deploy-source`); a v0.2 remove esse caminho e torna `Git → CI → OCI → Release
→ deployment` o único fluxo.

## Princípios

1. **Core não conhece interfaces** — CLI, TUI e API chamam os mesmos casos de uso.
2. **Idempotência** — repetir operação não duplica recurso.
3. **Estado desejado ≠ observado** — desejado no SQLite, observado no Podman.
4. **Aplicação sobrevive ao Pneuma** — runtime supervisionado pelo host.
5. **Release imutável** — identificada por digest OCI; deployment é tentativa.
6. **Exposição independente** — visibility e runtime são ortogonais.
7. **Pneuma é operável** — instalável, atualizável, diagnosticável, recuperável.

---

## v0.1 — Fundação OCI

Pneuma registra aplicações e implanta releases OCI imutáveis com segurança
operacional básica, preservando a versão saudável quando um novo deploy falha.

### Entidades-alvo

```text
System (novo)
└── Application
      ├── desired_runtime_state
      ├── desired_visibility
      ├── active_deployment_id
      ├── delivery_spec (type, image_repository)
      │
      └── Release (novo)
            └── Deployment
                  └── RuntimeInstance
```

### Já implementado

| Capacidade | Status | Notas |
|---|---|---|
| Application entity + catálogo | ✅ | |
| SQLite + migrations (13) | ✅ | |
| Deployment persistence + state machine | ✅ | |
| RuntimeInstance persistence | ✅ | |
| Podman rootless (create, start, stop, inspect) | ✅ | Build local removido na v0.2 |
| Start / stop / status | ✅ | |
| Internal health check | ✅ | |
| External health check | ✅ | |
| Safe traffic switch (falha preserva runtime saudável) | ✅ | |
| Caddy integration + public routing | ✅ | |
| Exposure materialization state | ✅ | |
| Deployment history | ✅ | |
| CLI (import, list, status, deploy, start, stop, visibility set, deployments, version) | ✅ | |
| Rollback (novo deployment da Release anterior, não depende do container antigo) | ✅ | |
| Visibility set (public/internal) independente do lifecycle | ✅ | |
| Doctor (13 checks: DB, migrations, workspace, Caddy dirs, Caddyfile/config, git, podman, rootless, Quadlet generator, OCI images, disk, caddy) | ✅ | |
| Version | ✅ | |
| Staging validation (`staging.vitoralmeida.tech`) | ✅ | |
| System (entidade, migration, CLI create/list/show) | ✅ | |
| Release imutável + engine DeployRelease | ✅ | DeploySource removido na v0.2 |
| OCI adapter (pull + digest) + `app deploy --image` | ✅ | |
| Manifest v2 com `[delivery]` + enforcement de repositório | ✅ | Substituído por schema v3 na v0.2 |

**Capacidades removidas na v0.2:**
- ~~`app deploy-source`~~ (build local removido)
- ~~`deployment_deploy_source.rs`~~ (engine de build local removido)
- ~~`local_build`~~ (módulo de build local removido)
- ~~`[source]` e `[build]` no manifesto~~ (removidos no schema v3)
- ~~Import por path local~~ (apenas Git remoto na v0.2)

### Pendente — 7 entregas

#### 1. System

- [x] Entidade `System` (id, name, description?, created_at)
- [x] `Application.system_id`
- [x] Migration `0005_systems`
- [x] CLI: `pneuma system create`, `system list`, `system show`

#### 2. Release + refatoração do engine de deploy

Introduzir Release, substituir candidate/current/previous e dividir
`deployment_deploy_internal.rs` (~1500 linhas) em dois caminhos com
responsabilidades claras.

**Domínio:**

- [x] Entidade `Release` (id, application_id, image_repository, image_digest, source_revision?, created_at)
- [x] Migration `0006_releases`
- [x] Migration `0007_deployment_release` (deployment referencia release, não mais revision)
- [x] Remover `RuntimeRole` (candidate/current/previous); RuntimeInstance ganha estados próprios: `starting | running | stopped | failed | removed`
- [x] `Application.active_deployment_id` substitui roles; deployment ativo → release ativa → runtime ativo
- [x] Deployment states: `pending | starting | verifying | activating | succeeded | failed` (remover `preparing_source`, `building`, `switching_traffic`, `verifying_external`)
- [x] Rollback cria novo deployment (type=rollback) a partir da Release anterior; não depende de container anterior existir

**Engine split:**

- [x] `DeployRelease` (`deployment_deploy_release.rs`): ensure image → create deployment → create runtime → start → verify → activate
- [x] Remover `reconcile_existing_runtime()` do deploy; mesma Release ativa → no-op, app parada → `app start`, release anterior → rollback
- [x] `DeploymentSpecification` simplificado: sem containerfile/context; apenas application_id, image, container_port, health_path, expected_status, visibility

**Nova estrutura de use_cases:**

```text
use_cases/
├── release_create.rs          ← cria Release (OCI)
├── deployment_deploy_oci.rs   ← DeployOci: pull/verifica → Release → DeployRelease
├── deployment_deploy_branch.rs← DeployByBranch: branch → commit → image tag → DeployOci
├── deployment_deploy_release.rs ← DeployRelease: orquestrador linear
├── deployment_start_candidate.rs ← criação do runtime candidato
├── deployment_activate_public.rs ← ativação pública (health + Caddy)
├── deployment_runtime_cleanup.rs ← cleanup de candidates e runtimes antigos
├── deployment_progress.rs        ← reporting de progresso
├── deployment_transition.rs   ← máquina de estados persistida
├── deployment_rollback.rs     ← rollback como novo deployment
├── application_runtime.rs     ← lifecycle start/stop/status
├── exposure_change.rs         ← public ↔ internal sem redeploy
└── ...
```

**Nota:** `deployment_deploy_source.rs` foi removido na v0.2 junto com o build local.

#### 3. OCI adapter

- [x] Adapter OCI: `podman pull`, `podman image inspect`, validar digest corresponde ao solicitado
- [x] `DeployRelease` passa a aceitar imagem de registry (além de imagem local do build)
- [x] CLI: `pneuma app deploy <app> --image <repo@sha256:...>` como caminho oficial
- [x] Rejeitar tags mutáveis (exigir digest)

#### 4. deploy-source (CLI) — REMOVIDO NA v0.2

- [x] CLI: `pneuma app deploy-source <app> <repo> --revision <rev>` (caminho alternativo)
- [x] Engine único: `DeploySource` já criado na entrega 2; aqui apenas expor na CLI

**Nota:** Este caminho foi removido na v0.2. O único artifact deployável agora é `image@digest` descoberto pelo CI.

#### 5. Manifesto com `[delivery]` — EVOLUÍDO PARA SCHEMA v3 NA v0.2

- [x] Seção `[delivery]` no manifesto: `type = "oci"`, `image = "ghcr.io/..."`
- [x] `[source]` e `[build]` tornam-se opcionais (apenas para deploy-source)
- [x] `schema_version = 2`
- [x] Persistir `application_delivery_specs` na importação
- [x] `app deploy --image` rejeita repositório diferente do permitido; `deploy-source` exige `[source]`/`[build]`

**Nota:** Na v0.2, o schema evoluiu para v3, removendo `[source]` e `[build]`. O repository vem do import, a branch vem do deploy.

#### 6. Histórico + visibility

- [x] Histórico baseado em Release/digest (não mais commit_sha)
- [x] Saída: `DEPLOYMENT | RELEASE | SOURCE | STATUS`
- [x] Renomear CLI: `app expose` → `app visibility set <app> public|internal`
- [x] Mensagens de saída alinhadas com o termo "visibility"

#### 7. Operabilidade final

- [x] Sobrevivência a reboot do host (Quadlet por deployment, habilitado após promoção)
- [x] Doctor estendido: rootless funcional, `caddy validate`, pull OCI ativo e espaço em disco
- [x] `pneuma database backup <path>`
- [x] `pneuma database restore <path>`
- [x] Docs atualizadas (roadmap, arquitetura, scope, README) refletindo OCI-first
- [x] E2E final: CI → GHCR → pull → deploy → health → active → rollback → reboot

**v0.1.0 concluída em 8 de agosto de 2026** — todos os critérios de aceite
foram validados na VPS de produção (`srv655252`, Debian 13). A v0.2
(Git-aware OCI Delivery) foi concluída em seguida — ver próxima seção.

### Modelo de dados alvo (v0.1)

```text
System
  id, name, description?, created_at

Application
  id, system_id, name, desired_runtime_state, desired_visibility,
  active_deployment_id, runtime_config, health_config, created_at, updated_at

Release
  id, application_id, image_repository, image_digest,
  source_revision?, created_at

Deployment
  id, application_id, release_id, type, status,
  requested_by, started_at, finished_at, failure_reason

RuntimeInstance
  id, deployment_id, runtime_identifier,
  state (starting|running|stopped|failed|removed),
  host_address, host_port, created_at
```

---

## v0.2 — Git-aware OCI Delivery

**Status:** concluída em 10 de agosto de 2026

O Pneuma passa de "opera uma imagem OCI que recebo" para "encontra o artifact do
commit de uma branch e o implanta". Pneuma deixa de construir aplicações: **CI
produz artifacts, Pneuma descobre e opera artifacts.**

```text
Git branch → commit → OCI digest → Release → deployment
```

Princípios e mudanças estruturais:

- **Remover build local:** `app deploy-source`, `deployment_deploy_source`,
  `local_build`, `[build]`, `application_build_specs` e checkout permanente de
  build. O único artifact deployável é `image@digest`.
- **Import apenas por Git remoto:** `pneuma app import <git-url>
  [--manifest <path>]` substitui o import por path local. Checkout somente
  temporário (clone → ler `pneuma.toml` → persistir → remover). `import` não
  faz deployment; `active_deployment_id = null`, runtime desejado = stopped.
- **Manifest schema v3:** sem `[source]`/`[build]`. Repository vem do import,
  branch vem do deploy, OCI/runtime/exposure vêm do manifesto. Convenção
  `deploy/<environment>/pneuma.toml` (dev/staging/production); environments
  ainda não são entidade do domínio.
- **Persistência:** regra arquitetural — use cases decidem o que deve acontecer,
  stores SQLite decidem como persistir atomicamente. `SqliteApplicationStore`,
  `SqliteDeploymentStore`, `SqliteRuntimeStore`, `SqliteReleaseStore`
  (orientados a capacidades, não repository por tabela). Reads simples (`app
  list`, `system list`, histórico) continuam queries diretas. Nunca abrir
  transação durante Git/registry/Podman/Caddy (I/O externo fora da transação;
  persistir em transação curta no fim).
- **Deploy por branch:** `pneuma app deploy <app> --branch <branch>`
  (mutuamente exclusivo com `--image`). Novo use case `DeployByBranch`
  (`deployment_deploy_branch.rs`): branch → `git ls-remote` → `CommitSha`
  (congelado para o deployment) → convenção `image:<commit-sha>` → resolver
  tag → digest → `DeployOci`. Se o CI ainda não publicou o artifact →
  `ArtifactNotFound`, sem fallback para `:latest`/anterior/build local.
- **Release correlaciona source e artifact:** `source_revision`, `image_repository`,
  `image_digest`, `image_reference`.
- **Fases de implementação (todas concluídas):**

  - A — simplificar: remover `deploy-source`, `deployment_deploy_source`,
    `local_build`, `[build]`, `application_build_specs`, import por path local,
    source local e checkout permanente.
  - B — separar persistência: criar os quatro SQLite stores e migrar
    create/transition/fail/promotion, runtime persistence, release/rollback.
  - C — novo schema: manifest v3, `deploy/<environment>/pneuma.toml`, novas
    migrations (nunca alterar as históricas).
  - D — import Git remoto: `app import <git-url>`, `--manifest`, checkout
    temporário, persistir `repository_url`/`manifest_path`, idempotência.
  - E — Git resolution: adapter de Git remoto, `resolve_branch()`, `CommitSha`,
    erros de auth/repositório/branch.
  - F — OCI discovery: convenção `image:<commit>`, resolver tag do commit →
    digest, nunca devolver tag mutável ao engine.
  - G — deploy por branch: `DeployByBranch`, `--branch`, exclusão mútua com
    `--image`, persistir `source_revision`.
  - H — aplicação real: mover manifestos do website, importar staging, testar
    `--branch staging`, automatizar staging no Actions, importar production,
    testar `--branch main` e rollback.

**Definition of Done:** `pneuma app import <git-url> --manifest
deploy/staging/pneuma.toml` seguido de `pneuma app deploy
vitoralmeida-tech-staging --branch staging` encontra e implanta o artifact
correto — sem clone manual na VPS, import por path, build local, `podman build`
pelo Pneuma, descoberta manual de digest ou edição manual de Caddy.

---

## v0.3 — Reconciliation & Deployment Reliability

Com Git/source/artifact bem definidos, o Pneuma evolui de command-driven para
declarativo (estado desejado vs observado). O `pneuma reconcile` observa o
estado materializado (Podman/systemd, Caddy) e converge para o estado desejado
persistido no SQLite, sem alterar a intenção e sem criar Release/Deployment.

- [ ] Desired vs observed state
- [ ] Drift detection e automatic recovery
- [ ] Deployment recovery
- [ ] Melhor convergência de restart/reboot
- [ ] Melhorias de candidate/activation
- [ ] Exclusão mútua de deployment (um por aplicação por vez)
- [ ] CLI não interativa (`--non-interactive`, output estruturado, exit codes)

Já entregues fora da v0.3 (não são trabalho futuro):

- GitHub Actions de validação (format, lint, test, build) e build/push GHCR
  publicado como `image:<commit-sha>` — pipeline de CI ativo.
- Deploy SSH via dispatcher restrito: GitHub Actions → chave exclusiva →
  usuário `pneuma` (sem senha, sem sudo), `authorized_keys` com forced command
  (`pneuma ci dispatch`) limitado a `deploy <app> <branch>` e `version`.

Fora de escopo sem necessidade demonstrada: registry watcher (deploy quando o
artifact da branch fica disponível) e automatic deploy policies. Sem auditoria
completa, `--idempotency-key` genérica, retenção de imagens ou rollback
automático posterior à promoção nesta etapa — a falha do candidate antes da
promoção já preserva a versão ativa; decisão de reverter automaticamente uma
versão já promovida fica para uma política explícita futura.

---

## v0.4 — Application Topology & Internal Networking

Adicionar relacionamento entre Applications: o Pneuma passa a entender como as
aplicações se relacionam, não apenas como cada uma roda isoladamente.

- [ ] Service relationships (`Application A depends on Application B`)
- [ ] Internal services
- [ ] Application dependencies
- [ ] Network/service addressing
- [ ] System como agrupador real
- [ ] Service discovery básico

---

## v0.5 — Network Policy Enforcement

As relações declaradas na v0.4 alimentam políticas de conectividade aplicadas
no host.

### Network enforcement

- [ ] `pneuma-netd` (nftables, default deny, conectividade explícita)

---

## v0.6 — Workload Identity & Secure S2S

Identidade por workload; a comunicação entre aplicações passa a ser
autenticada e autorizada.

- [ ] SPIFFE + SPIRE (cada `RuntimeInstance` recebe identidade própria)
- [ ] `pneuma-proxy` por `RuntimeInstance` (mTLS, authn, authz, telemetria)

---

## v0.7 — Artifact Security & Secrets

Segurança do ciclo de artefato e segredos de aplicação.

### Artifact security

- [ ] SBOM generation e enforcement
- [ ] Verificação de assinatura de imagem (cosign/Notation)
- [ ] Admission policies (rejeitar imagem não assinada)
- [ ] Gestão de secrets (injeção, rotação)
- [ ] Threat model implementado

---

## Fora do escopo (congelado além da v0.7)

TUI, API HTTP, webhooks, observabilidade centralizada, múltiplos hosts,
scheduler, agentes remotos, reconciliação distribuída, comunicação declarativa
entre apps, dependencies, service discovery além do básico da v0.4, managed
builds como feature oficial, canary, rollout gradual, autoscaling, Kubernetes,
RBAC, multiusuário.

Os itens de rede, identidade, S2S e segurança de artifact saem do congelamento
nas versões que os introduzem (v0.5 a v0.7); todo o restante é descongelado
explicitamente em uma versão futura.
