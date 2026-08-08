# Roadmap consolidado do Pneuma — v0.1 a v0.6

**Status:** documento vivo — contrato de evolução do projeto
**Aplicação-piloto:** `vitoralmeida.tech`

## Fluxo unificado (v0.1+)

```text
                    ┌─ OCI Registry (ghcr.io)
                    │   image@sha256:...
                    ▼
               Create Release
                    │
                    ▼
               DeployRelease
                    ▲
                    │
                    └─ Local build (deploy-source)
                         source → build → Release
```

O CI produz o artefato. O Pneuma recebe o artefato e o opera. Ambos os caminhos
produzem uma Release e passam pelo mesmo engine de deployment.

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
      │
      └── Release (novo)
            └── Deployment
                  └── RuntimeInstance
```

### Já implementado

| Capacidade | Status |
|---|---|
| Application entity + catálogo | ✅ |
| SQLite + migrations (4) | ✅ |
| Deployment persistence + state machine | ✅ |
| RuntimeInstance persistence | ✅ |
| Podman rootless (build, create, start, stop, inspect) | ✅ |
| Start / stop / status | ✅ |
| Internal health check | ✅ |
| External health check | ✅ |
| Safe traffic switch (falha preserva runtime saudável) | ✅ |
| Caddy integration + public routing | ✅ |
| Exposure materialization state | ✅ |
| Deployment history | ✅ |
| Local Git checkout + local OCI build | ✅ |
| CLI (import, list, status, deploy, start, stop, expose, deployments, version) | ✅ |
| Rollback (reusa deployment antigo, troca roles) | ✅ |
| Expose public/internal (funcional, nome antigo) | ✅ |
| Doctor (8 checks: DB, migrations, workspace, Caddy dirs, Caddyfile, git, podman, caddy) | ✅ |
| Version | ✅ |
| Staging validation (`staging.vitoralmeida.tech`) | ✅ |

### Pendente — 7 entregas

#### 1. System

- [ ] Entidade `System` (id, name, description?, created_at)
- [ ] `Application.system_id`
- [ ] Migration `0005_systems`
- [ ] CLI: `pneuma system create`, `system list`, `system show`

#### 2. Release + refatoração do engine de deploy

Introduzir Release, substituir candidate/current/previous e dividir
`deployment_deploy_internal.rs` (~1500 linhas) em dois caminhos com
responsabilidades claras.

**Domínio:**

- [ ] Entidade `Release` (id, application_id, image_repository, image_digest, source_revision?, created_at)
- [ ] Migration `0006_releases`
- [ ] Migration `0007_deployment_release` (deployment referencia release, não mais revision)
- [ ] Remover `RuntimeRole` (candidate/current/previous); RuntimeInstance ganha estados próprios: `starting | running | stopped | failed | removed`
- [ ] `Application.active_deployment_id` substitui roles; deployment ativo → release ativa → runtime ativo
- [ ] Deployment states: `pending | starting | verifying | activating | succeeded | failed` (remover `preparing_source`, `building`, `switching_traffic`, `verifying_external`)
- [ ] Rollback cria novo deployment (type=rollback) a partir da Release anterior; não depende de container anterior existir

**Engine split:**

- [ ] `DeploySource` (`release_build_source.rs`): resolve Git → checkout → build → cria Release local → chama `DeployRelease`
- [ ] `DeployRelease` (`deployment_deploy.rs`): ensure image → create deployment → create runtime → start → verify → activate
- [ ] Remover `reconcile_existing_runtime()` do deploy; mesma Release ativa → no-op, app parada → `app start`, release anterior → rollback
- [ ] `DeploymentSpecification` simplificado: sem containerfile/context; apenas application_id, image, container_port, health_path, expected_status, visibility

**Nova estrutura de use_cases:**

```text
use_cases/
├── release_create.rs          ← cria Release (OCI ou local)
├── release_build_source.rs    ← DeploySource: git → build → Release → DeployRelease
├── deployment_deploy.rs       ← DeployRelease: orquestrador linear
├── deployment_activate.rs     ← ativação (internal: mark active; public: Caddy + health externo)
├── deployment_fail.rs         ← persistência de falha + cleanup
├── deployment_rollback.rs     ← rollback como novo deployment
├── runtime_start.rs           ← lifecycle start
├── runtime_stop.rs            ← lifecycle stop
└── visibility_change.rs       ← public ↔ internal sem redeploy
```

#### 3. OCI adapter

- [ ] Adapter OCI: `podman pull`, `podman image inspect`, validar digest corresponde ao solicitado
- [ ] `DeployRelease` passa a aceitar imagem de registry (além de imagem local do build)
- [ ] CLI: `pneuma app deploy <app> --image <repo@sha256:...>` como caminho oficial
- [ ] Rejeitar tags mutáveis (exigir digest)

#### 4. deploy-source (CLI)

- [ ] CLI: `pneuma app deploy-source <app> <repo> --revision <rev>` (caminho alternativo)
- [ ] Engine único: `DeploySource` já criado na entrega 2; aqui apenas expor na CLI

#### 5. Manifesto com `[delivery]`

- [ ] Seção `[delivery]` no manifesto: `type = "oci"`, `image = "ghcr.io/..."`
- [ ] `[source]` e `[build]` tornam-se opcionais (apenas para deploy-source)
- [ ] `schema_version = 2`

#### 6. Histórico + visibility

- [ ] Histórico baseado em Release/digest (não mais commit_sha)
- [ ] Saída: `DEPLOYMENT | RELEASE | SOURCE | STATUS`
- [ ] Renomear CLI: `app expose` → `app visibility set <app> public|internal`
- [ ] Mensagens de saída alinhadas com o termo "visibility"

#### 7. Operabilidade final

- [ ] Sobrevivência a reboot do host (systemd/Quadlet para containers current)
- [ ] Doctor estendido: rootless funcional, caddy validate, registry/pull, espaço em disco
- [ ] `pneuma database backup <path>`
- [ ] `pneuma database restore <path>`
- [ ] Docs atualizadas (roadmap, arquitetura, scope, README) refletindo OCI-first
- [ ] E2E final: CI → GHCR → pull → deploy → health → active → rollback → reboot

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

## v0.2 — CI/CD automatizado

CI produz imagem e aciona deployment via SSH.

- [ ] GitHub Actions: workflow de validação (PR: format, lint, test, build)
- [ ] GitHub Actions: build + push GHCR (merge na main)
- [ ] CLI não interativa (`--non-interactive`, output estruturado, exit codes)
- [ ] Exclusão mútua de deployment (um por aplicação por vez)
- [ ] Chave de idempotência (`--idempotency-key`)
- [ ] Usuário SSH dedicado (`pneuma-deployer`, forced command)
- [ ] GitHub Actions: stage de deploy (SSH → `pneuma app deploy`)
- [ ] Rollback automático em falha de health check
- [ ] Auditoria completa (workflow, run ID, timestamps, requested_by)
- [ ] Política de retenção de imagens

---

## v0.3 — Deploy automático

Merge na main termina com nova versão implantada e verificada.

- [ ] Pipeline completo: merge → CI → SSH → deploy → health → active
- [ ] Caddy atômico (temp → validate → swap → reload → verify externo)
- [ ] External health check pós-troca com auto-rollback
- [ ] Container candidato (blue-green simplificado)
- [ ] Preflight checks antes da troca de runtime
- [ ] Descoberta de releases (`pneuma release refresh`)
- [ ] Release list/status na CLI

---

## v0.4 — Multi-app e inter-app

Sistemas com múltiplas aplicações comunicando-se.

- [ ] Topologia de System (relações declaradas entre aplicações)
- [ ] Ordem de deploy por dependência
- [ ] Service discovery gerenciado (DNS ou injeção de ambiente)
- [ ] Redes compartilhadas entre aplicações do mesmo System
- [ ] Configuração de comunicação app-to-app
- [ ] Visão e operação no nível de System

---

## v0.5 — Segurança e identidade

Identidade de workload e proteção de comunicação.

- [ ] mTLS entre aplicações (pneuma-proxy sidecar)
- [ ] SPIFFE/SPIRE workload identity
- [ ] Gestão de secrets (injeção, rotação)
- [ ] SBOM generation e enforcement
- [ ] Verificação de assinatura de imagem (cosign/Notation)
- [ ] Admission policies (rejeitar imagem não assinada)
- [ ] Network policies (nftables/pneuma-netd)
- [ ] Threat model implementado

---

## v0.6 — API e observabilidade

Interfaces remotas e monitoramento.

- [ ] HTTP API (REST, mesmos casos de uso da CLI)
- [ ] Webhooks (triggers de deploy, notificações de status)
- [ ] Registry watcher (auto-descoberta de novas imagens)
- [ ] Observabilidade básica (métricas, logs estruturados)
- [ ] TUI (interface interativa no terminal)
- [ ] Multi-host (scheduler, agentes remotos)
- [ ] Reconciliação contínua (desejado vs observado)

---

## Fora do escopo (congelado até v0.6)

TUI (exceto v0.6), API HTTP (exceto v0.6), webhooks, registry watcher, múltiplos
hosts, scheduler, comunicação declarativa entre apps, dependencies, service
discovery, pneuma-netd, nftables, network policies, SPIFFE/SPIRE, workload
identity, pneuma-proxy, mTLS, secrets, SBOM, signature enforcement, admission
policies, reconciliação contínua, managed builds como feature oficial, canary,
rollout gradual, autoscaling, Kubernetes, RBAC, multiusuário.

Cada item é descongelado na versão que o introduz explicitamente.
