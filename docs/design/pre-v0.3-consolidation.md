# Plano — Consolidação Pós-v0.2 / Pré-v0.3

**Status:** documento vivo — plano de trabalho para ser executado e atualizado.

**Fonte:** `~/Downloads/pneuma-post-v0.2-consolidation.md` e análise do código
atual (v0.2.0, commit base `c475c0a`).

**Regra de retomada:** se uma etapa falhar ou for interrompida, retomar a partir
do item pendente desta lista. Cada commit deve sair dos quatro gates do
`AGENTS.md` verde (`cargo fmt --check`, `clippy --all-targets --all-features -- -D warnings`,
`cargo test --all-features`, `cargo build --release`).

## Decisões fixadas

1. **Identidade de runtime sem source revision:** quando uma `Release` não tiver
   `source_revision` (deploy OCI sem commit Git), a identidade materializada no
   runtime e em `exposures.configuration_version` será **`image_digest`** —
   nunca `release.id`.
2. **Sem migration.** A identidade é derivada de dados imutáveis já persistidos;
   `source_revision` continua nullable e significa exclusivamente revisão Git.
3. **Sem novas abstrações.** Nada de `Repository<T>`, traits, Unit of Work, DI,
   service layer, event bus ou async. O objetivo é `SQL saiu do use case`, não
   redesenhar a persistência.
4. **Não mexer em CI/bootstrap** além de regressão, documentação e pequenos fixes.
5. **Não implementar `pneuma reconcile`** nesta etapa; apenas congelar a semântica
   em design document.

---

## Sequência de commits

1. `docs: redefine roadmap after v0.2`
2. `docs: open pre-v0.3 consolidation iteration`
3. `refactor(domain): make deployment and runtime first-class types`
4. `refactor(release): use the domain Release in the release store`
5. `refactor(deployment): rename deployed release result`
6. `refactor(runtime): use artifact identity for non-Git releases`
7. `refactor(store): move application import persistence to application store`
8. `refactor(store): move application runtime persistence to stores`
9. `refactor(store): move exposure persistence to application store`
10. `refactor(store): finish deployment creation persistence extraction`
11. `test(deployment): verify concurrent deployment exclusion`
12. `docs: define reconciliation semantics and invariants`
13. `docs: define v0.3 reconciliation e2e scenarios`
14. `docs: record pre-v0.3 consolidation completion`

Opcional: `release: publish v0.2.1 consolidated baseline`. Só depois: primeiro
commit funcional da v0.3 (`feat(reconcile): add one-shot application
reconciliation`).

---

## Passo 0 — Documentação de direção

### 0.1 Roadmap (`docs/roadmap.md`)

- [ ] Renomear v0.3 para **Reconciliation & Deployment Reliability** (não mais
      "Reconciliation, automation & CI/CD").
- [ ] Remover do escopo futuro de v0.3 os itens já entregues: GitHub Actions
      build/push, SSH deployment, forced command, CI dispatcher, usuário
      `pneuma-deployer`, registry watcher e automatic deploy policies.
- [ ] Reordenar versões futuras:
      v0.4 Application Topology & Internal Networking;
      v0.5 Network Policy Enforcement;
      v0.6 Workload Identity & Secure S2S;
      v0.7 Artifact Security & Secrets.
- [ ] Reformular `rollback automático em health failure` para distinguir:
      candidate failure antes da promoção; rollback automático após promoção.
- [ ] Corrigir a nota de que "import por path local" foi removido na v0.2 —
      `main.rs:599-611` ainda o aceita para cenários locais/testes.

### 0.2 Iteração formal (`docs/iterations/current-iteration.md`)

- [ ] Substituir o conteúdo concluído e abrir a iteração
      **Pre-v0.3 — Domain and Persistence Consolidation**.
- [ ] Registrar Definition of Done verificável:
      roadmap atualizado; Release/Deployment/Runtime consolidados; SQL extraído
      dos quatro use cases; `source_revision` corrigido; exclusão de deployments
      concorrentes testada; design de reconciliation existente; cenários E2E
      v0.3 definidos; regressão v0.2 verde.
- [ ] Ao encerrar, marcar caixas e commitar apenas esse arquivo como `docs:`.

### 0.3 Design de reconciliation (`docs/design/reconciliation.md`, novo)

- [ ] Criar o diretório/arquivo e adicionar ao índice `docs/README.md`.
- [ ] Fontes de verdade: SQLite = intenção + histórico + identidade lógica;
      Podman/systemd = runtime observado; Caddy = exposição observada;
      OCI registry = artifact disponível.
- [ ] Matriz runtime Desired × Observed:
      Running/Running no-op; Running/Stopped start; Running/Missing recover;
      Running/Failed recover/report; Stopped/Running stop; Stopped/Stopped
      no-op; Stopped/Missing no-op.
- [ ] Matriz de exposure: Public/correto no-op; Public/ausente materialize;
      Public/divergente replace; Internal/ausente no-op; Internal/presente remove.
- [ ] Política de deployment recovery para Pending/Starting/Verifying/Activating:
      conservadora — não promover ambíguo, preservar runtime anterior saudável,
      cleanup seguro, marcar interrupted/failed.
- [ ] Invariantes: máximo um Deployment ativo por Application; Release imutável;
      reconcile não cria Release; runtime recovery não cria Deployment novo por
      padrão; reconcile não muda desired state; não escolhe versão nova; não
      observa registry procurando Release nova; deve ser idempotente.

### 0.4 Arquitetura e README

- [ ] `docs/architecture/architecture.md`: declarar ownership — use case decide e
      controla fronteira transacional; store possui SQL e mapping; adapters
      externos possuem Git/OCI/Podman/systemd/Caddy. Manter a regra de nunca
      abrir transação durante I/O externo.
- [ ] `README.md`: corrigir descrição de v0.3 (não mais CI/CD).

---

## Passo 1 — Deployment e Runtime como tipos de domínio

- [ ] Criar `src/domain/deployment.rs` com `Deployment`, `DeploymentType`,
      `DeploymentStatus` (mover de `src/use_cases/deployment_create.rs:10-75`).
      Preservar os helpers `database_value`/`from_database` como `pub(crate)`.
- [ ] Criar `src/domain/runtime.rs` com `RuntimeState` (mover de
      `src/use_cases/deployment_create.rs:78-108`).
- [ ] Exportar `deployment` e `runtime` em `src/domain/mod.rs`.
- [ ] Atualizar imports de produção:
      `deployment_store.rs`, `deployment_list.rs`, `deployment_transition.rs`,
      `deployment_progress.rs`, `deployment_deploy_release.rs`,
      `deployment_deploy_oci.rs`, `deployment_rollback.rs`,
      `deployment_promote_internal.rs`, `deployment_promote_public.rs`,
      `deployment_activate_public.rs` (linha 119-122 com path qualificado).
- [ ] Atualizar imports de testes:
      `tests/deployment_create.rs`, `tests/deployment_transition.rs`,
      `tests/deployment_list.rs`, `tests/deployment_promote_internal.rs`,
      `tests/deployment_register_runtime.rs`.
- [ ] Remover os quatro tipos de `deployment_create.rs` (que passa a conter só
      criação, erros e regras).
- [ ] Não confundir com `ObservedRuntimeState` de `src/adapters/local_runtime.rs`
      (continua no adapter).
- [ ] Validar mudança puramente estrutural: sem alteração de schema, valores SQL,
      máquina de estados ou saída CLI.

---

## Passo 2 — Unificar o tipo Release

- [ ] Em `src/adapters/stores/release_store.rs`: importar
      `crate::domain::release::Release`; remover a struct local (linhas 38-47).
- [ ] `load_release_by_digest` retorna diretamente `domain::release::Release`.
- [ ] Em `src/use_cases/release_create.rs`: remover a conversão manual campo a
      campo (linhas 97-112).
- [ ] Manter idempotência por `(application_id, image_digest)` e semântica de
      `source_revision` (retida do primeiro insert).

---

## Passo 3 — Renomear `DeployedRelease` → `DeploymentResult`

- [ ] Em `src/use_cases/deployment_deploy_release.rs`: renomear a struct
      (linhas 29-37) e todos os usos internos.
- [ ] Atualizar tipos de retorno e imports em:
      `deployment_deploy_oci.rs`, `deployment_deploy_branch.rs`,
      `deployment_rollback.rs`.
- [ ] Manter os campos: `deployment_id`, `runtime_id`, `container_name`,
      `image_reference`, `source_revision`, `finished_at`.
- [ ] Não alterar a saída CLI em `main.rs:839-855,890-907,935-945`.

---

## Passo 4 — Semântica de `source_revision`

- [ ] Em `src/use_cases/deployment_deploy_release.rs:210`, substituir
      `release.source_revision.as_deref().unwrap_or(&release.id)` por identidade
      de runtime derivada: `source_revision` quando existir, senão `image_digest`.
- [ ] Renomear parâmetro interno para `runtime_identity` (execution,
      `CandidateStartInput`, escrita da label em
      `src/adapters/systemd_quadlet.rs:92-108`).
- [ ] A label Quadlet pode permanecer `io.pneuma.revision`, mas o valor nunca
      pode ser `release.id`.
- [ ] Passar a mesma identidade à promoção pública
      (`deployment_activate_public.rs:216` → `exposures.configuration_version`).
- [ ] `Release.source_revision` e `DeploymentResult.source_revision` continuam
      `None` no caso OCI sem Git.
- [ ] Testes: OCI sem source revision grava digest na label/configuração (nunca
      `release.id`); deploy por branch mantém SHA como source_revision e
      identidade materializada.

---

## Passo 5 — Extrair persistência de `application_import`

- [ ] Manter no use case: leitura/validação do manifest, decisão de
      `repository_kind`, e a transação única (atomicidade do agregado).
- [ ] Mover SQL para `src/adapters/stores/application_store.rs`, reutilizando
      métodos existentes: `generate_id`, `ensure_system`,
      `load_system_id_by_name`, `insert_application`, `insert_delivery_spec`,
      `insert_source_spec`, `insert_runtime_spec`, `insert_health_check_spec`,
      `insert_exposure`.
- [ ] Adicionar `load_application_for_import` (no `application_store`) que
      preserva o `LEFT JOIN application_sources` e retorna a aplicação
      pré-existente no import idempotente.
- [ ] Remover todo SQL inline de `application_import.rs`.
- [ ] Nenhuma chamada externa dentro da transação (hoje já é verdade).
- [ ] Regressão: `tests/application_import.rs` (spec completa, idempotência,
      falha de manifest, source kind, OCI sem source, system).

---

## Passo 6 — Extrair persistência de `application_runtime`

- [ ] Manter no use case: decisão desired×observed, persistência do desired
      state antes de controlar systemd/Podman, orquestração observe/resolve/
      start/stop/re-observa, mapeamento de erros (`RuntimeChanged`).
- [ ] Em `src/adapters/stores/runtime_store.rs`, adicionar:
  - [ ] `load_current_successful_runtime` (runtime do deployment ativo com
        status succeeded; inclui external id, porta, deployment id);
  - [ ] `reconcile_external_runtime_id` com `removed_at IS NULL` e row-count;
  - [ ] `persist_observation` (Missing | Observed{state, host_port}) que sempre
        grava `last_observed_at`, aplica `removed_at IS NULL` e retorna se
        atualizou 1 linha (preserva `RuntimeChanged`).
- [ ] Dedução `Missing` pós-stop de Quadlet = `Stopped` sem `removed_at`
      (persist_stopped_without_removal).
- [ ] Reutilizar `application_store::{load_desired_runtime_state,
      update_desired_runtime_state}`.
- [ ] Remover SQL inline de `application_runtime.rs` (linhas 371-523).
- [ ] Preservar fronteiras: reads antes de observar; desired antes de efeito;
      observação após efeito; nenhuma transação durante I/O externo.
- [ ] Testes de caracterização: runtime ativo só de deployment succeeded;
      desired persiste se controle falhar; reconciliação não revive removido;
      Missing+Stopped sem removed_at; Missing+Running → ContainerMissing;
      update sem linha → RuntimeChanged; running persiste porta e timestamp.

---

## Passo 7 — Extrair persistência de `exposure_change`

- [ ] Manter no use case: decisão public/internal, observação de runtime,
      materialização/validação/reload Caddy, health externo e restauração em
      falha, ordem dos efeitos e persistência final.
- [ ] Em `application_store.rs`:
  - [ ] Adicionar `load_exposure` (desired_visibility + domain opcional);
        exposure ausente → Internal (default atual).
  - [ ] Reutilizar `application_exists`, `load_exposure_domain`,
        `load_runtime_endpoint_for_exposure`.
  - [ ] Variantes transacionais `update_exposure_public_in_transaction` e
        `update_exposure_internal_in_transaction` (recebem `&Transaction<'_>`).
- [ ] Manter `TransactionBehavior::Immediate` no use case após sucesso de
      Caddy/health (public) e após remoção do fragmento (internal).
- [ ] Remover SQL inline de `exposure_change.rs` (linhas 131-330).
- [ ] Testes: app inexistente; public sem runtime ativo; public sem domain;
      health falha restaura fragmento e não persiste public; internal idempotente
      com `domain = NULL`.

---

## Passo 8 — Finalizar extração de `deployment_create`

- [ ] Manter no use case: `TransactionBehavior::Immediate`, regras
      (`ApplicationNotFound`, `ReleaseNotFound`, `ActiveDeployment`,
      `AlreadyActive`) e commit.
- [ ] Em `src/adapters/stores/deployment_store.rs`, adicionar:
  - [ ] `has_nonterminal_deployment`;
  - [ ] `load_active_runtime_release_id` (preservar predicados: active
        deployment, runtime running/stopped, `removed_at IS NULL`);
  - [ ] `insert_pending_deployment`;
  - [ ] `load_deployment`.
- [ ] Rollback da release já ativa continua permitido.
- [ ] Remover helpers SQL privados de `deployment_create.rs`.
- [ ] Reforçar `tests/deployment_create.rs`: ativa running → AlreadyActive;
      ativa stopped → AlreadyActive; runtime removido não bloqueia; rollback da
      release atual permitido.

---

## Passo 9 — Provar exclusão mútua real

- [ ] Em `tests/cli.rs`, teste com **dois processos separados** (não threads)
      compartilhando um `DeploymentEnvironment`.
- [ ] Estender o fake `systemctl` com gate por variável de ambiente (somente no
      teste): processo A chega ao start da unit, escreve marcador e espera
      liberação — o deployment A já está committed como não-terminal.
- [ ] Processo B contra o mesmo DB → erro CLI de `ActiveDeployment`.
- [ ] Liberar A → sucesso do primeiro deploy.
- [ ] Sem lock novo: o teste prova o mecanismo atual.

---

## Passo 10 — Regressão final

- [ ] Quatro gates verdes.
- [ ] VM Debian 13 limpa: bootstrap → doctor → system create → app import →
      branch deploy → candidate → health → promotion → status → stop/start →
      visibility → rollback → reboot → CI dispatcher.
- [ ] `scripts/dev-vm/test-all.sh` verde (baseline 27 PASS / 0 FAIL / 1 SKIP).

---

## Fora de escopo (não bloquear)

Registry watcher; idempotency-key genérica; audit trail completo do GitHub;
image retention; API HTTP; TUI; OIDC; GitHub App; RBAC; novo usuário Linux;
`pneuma reconcile` (implementação).

## Definition of Done (perguntas-resposta "sim")

- Existe definição única de Release? Deployment é primeira classe? Runtime é
  primeira classe? `DeployedRelease` não borra mais Release/Deployment?
  `source_revision` significa só revisão de source? Use cases do futuro
  reconciler estão sem SQL direto? Quem controla transaction/SQL está claro?
  Deployment concorrente está testado? Fontes de verdade documentadas? Existe
  matriz desired×observed? Existe política para Deployment interrompido?
  Non-goals do reconciler documentados? Regressão v0.2 verde?
