# Plano — Consolidação Pós-v0.2 / Pré-v0.3

**Status:** design aprovado; a execução diária e o progresso vivem somente em
`docs/iterations/current-iteration.md`.

**Base revisada:** commit `c7db4f1`.

**Objetivo:** consolidar o modelo de domínio, a persistência e a semântica que
o reconciler reutilizará. Esta etapa não implementa `pneuma reconcile` nem
introduz uma capacidade de produto nova.

**Regra de retomada:** se uma etapa falhar ou for interrompida, retomar pelo
primeiro item pendente da sequência de commits. Cada commit de código deve sair
dos quatro gates do `AGENTS.md` verde:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --workspace --release
```

## Decisões fixadas

1. **Artifact identity:** `image_digest` é sempre a identidade canônica do
   artifact e do runtime materializado. Commit Git não substitui o digest.
2. **Proveniência Git:** `source_revision` pertence a cada `Deployment`, não à
   `Release`. Uma Release representa apenas o artifact imutável; deployments
   distintos podem apontar para o mesmo digest e ter proveniências distintas.
3. **Configuração Caddy:** `exposures.configuration_version` identifica a rota
   materializada, usando uma representação canônica do fragmento Caddy
   (`domain` + endpoint). Não identifica artifact nem deployment.
4. **Import público:** `pneuma app import` aceita somente URLs Git remotas,
   incluindo `file://` para testes locais. Paths locais deixam de ser aceitos
   pela CLI. `import_application` pode continuar recebendo um checkout local,
   pois é o use case interno posterior à resolução/clonagem.
5. **Recovery de runtime:** o reconciler recupera container ou Quadlet ausente
   reutilizando a mesma `RuntimeInstance` lógica e o mesmo Deployment. Uma
   observação `Missing` não grava `removed_at`; esse tombstone é reservado a
   retirement e cleanup intencionais.
6. **Visibility é intenção:** `visibility set public|internal` persiste a
   intenção solicitada antes de alterar Caddy. Falhas mantêm a intenção e
   registram `failed` ou `diverged` para futura convergência.
7. **Compatibilidade v0.2:** Quadlets e `configuration_version` existentes que
   contêm `release.id` continuam aceitos até o próximo redeploy ou
   rematerialização. A regra nova vale para materializações novas; não haverá
   reparo automático dos arquivos/containers legados nesta etapa.
8. **Sem abstrações genéricas:** nada de `Repository<T>`, traits, Unit of Work,
   DI, service layer, event bus ou async. Use cases controlam orquestração e
   transações; stores possuem SQL, mapping de rows e compare-and-set.
9. **Sem reconciler ainda:** a semântica e os cenários de teste são definidos
   antes da implementação. O primeiro comando `pneuma reconcile` é trabalho
   posterior a esta consolidação.

---

## Modelo alvo

```text
Release
= artifact OCI imutável
= application_id + image_repository + image_reference + image_digest

Deployment
= tentativa de ativar uma Release
= type + status + source_revision? + histórico da tentativa

RuntimeInstance
= materialização concreta de um Deployment
= é recuperada sem criar Deployment novo quando houver drift recuperável

Application
= desired runtime state + desired visibility + active deployment

SQLite
= intenção + histórico + identidade lógica

Podman/systemd/Caddy
= estado observado/materializado
```

### Ownership arquitetural

```text
Use case owns:
- regra de negócio
- orquestração e ordenação
- fronteira de transaction quando várias escritas precisam ser atômicas
- decisão de compensação após efeitos externos

Store owns:
- SQL
- mapping row <-> domínio
- serialização de valores persistidos
- primitives de persistência e compare-and-set

External adapter owns:
- Git, OCI, Podman, systemd e Caddy
```

Uma transaction SQLite nunca pode permanecer aberta durante Git, OCI, Podman,
systemd, Caddy ou HTTP.

**Escopo desta limpeza de SQL:** `application_import`, `application_runtime`,
`exposure_change` e `deployment_create` devem terminar sem SQL inline. SQL
mutável restante nos fluxos de promoção, registro de runtime e criação de
sistema é dívida explícita; não deve ser escondido por uma afirmação universal
de que todo use case já está livre de SQL.

---

## Sequência de commits

`docs: revise pre-v0.3 consolidation decisions` (`99fd151`) aprovou este
design antes de abrir a iteração. A partir daqui, a sequência é:

1. `docs: open pre-v0.3 consolidation iteration`
2. `docs: redefine roadmap after v0.2`
3. `docs: define reconciliation semantics before refactoring`
4. `fix(import): enforce Git URL application imports`
5. `refactor(domain): make deployment and runtime state first-class`
6. `refactor(persistence): move source provenance to deployments`
7. `refactor(release): use a single artifact-only Release type`
8. `refactor(deployment): rename deployed release result`
9. `refactor(runtime): materialize image digest as runtime identity`
10. `refactor(store): move application import persistence to application store`
11. `refactor(store): move application runtime persistence to stores`
12. `refactor(exposure): persist desired visibility before materialization`
13. `refactor(store): finish deployment creation persistence extraction`
14. `test(deployment): verify immediate locking`
15. `test(deployment): reject a second CLI deploy in progress`
16. `docs: define v0.3 reconciliation e2e scenarios`
17. `docs: record pre-v0.3 consolidation completion`

Opcionalmente, publicar `v0.2.1` como baseline consolidada. Somente então pode
começar `feat(reconcile): add one-shot application reconciliation`.

---

## Passo 0 — Documentação e governança

### 0.1 Consolidar os documentos de planejamento

- [ ] Manter este arquivo como design aprovado, sem checkboxes de execução.
- [ ] Abrir `docs/iterations/current-iteration.md` como o único tracker da
      execução, com as etapas e critérios verificáveis abaixo.
- [ ] Arquivar ou remover o papel autoritativo de `docs/next-iteration.md`, que
      é um rascunho duplicado e não está indexado no `docs/README.md`.
- [ ] Garantir que `docs/README.md` só indexe documentos vivos com um papel
      explícito.

### 0.2 Atualizar roadmap e documentação pública

- [ ] Em `docs/roadmap.md`, redefinir:

```text
v0.1 — OCI Deployment Foundation         concluída
v0.2 — Git-aware OCI Delivery            concluída
v0.3 — Reconciliation & Deployment Reliability
v0.4 — Application Topology & Internal Networking
v0.5 — Network Policy Enforcement
v0.6 — Workload Identity & Secure S2S
v0.7 — Artifact Security & Secrets
```

- [ ] Remover da v0.3 itens já entregues: GitHub Actions build/push, deploy
      SSH, forced command, CI dispatcher e usuário de deploy dedicado.
- [ ] Manter registry watcher e automatic deploy policies fora de escopo, sem
      necessidade demonstrada.
- [ ] Distinguir falha do candidate antes da promoção de rollback automático
      posterior à promoção.
- [ ] Atualizar título e seção "além da v0.5" do roadmap para refletir v0.7.
- [ ] Atualizar `README.md`, arquitetura e inventário de módulos quando os
      novos módulos de domínio forem criados.
- [ ] Corrigir todos os documentos e scripts que ainda apresentam paths locais
      como import público: `scripts/dev-vm/deploy-all-fixtures.sh` e
      `scripts/verify-vps.sh` são exemplos atuais.
- [ ] Corrigir a referência quebrada de `scripts/dev-vm/test-all.sh` para
      `docs/operations/e2e-testing.md`, criando o documento ou removendo a
      referência.

---

## Passo 1 — Definir reconciliation antes dos refactors

Criar `docs/design/reconciliation.md`, indexado em `docs/README.md`, antes de
projetar as APIs de runtime e exposure.

### 1.1 Fontes de verdade

```text
SQLite          intenção, histórico, identidades lógicas
Podman/systemd  runtime e unidade observados
Caddy           rota observada/materializada
OCI registry    disponibilidade de artifact, não seleção de versão pelo reconcile
```

O reconciler não consulta registry para escolher uma Release nova.

### 1.2 Runtime desired × observed

A matriz deve incluir estado, identidade e materialização. `Running/Running`
só é no-op quando deployment ativo, image digest, labels, porta, unit Quadlet e
container correspondem ao estado esperado.

| Desired | Observed | Ação |
|---|---|---|
| Running | Running e identidade/configuração corretas | no-op |
| Running | Stopped, unit presente | start unit |
| Running | Missing, unit presente | start unit e reconciliar external runtime id |
| Running | Missing, unit ausente | rematerializar unit, iniciar e reconciliar external runtime id |
| Running | Failed | recuperar ou reportar conforme diagnóstico seguro |
| Running | Running com digest/label/porta/unit divergente | não alterar destrutivamente sem regra explícita; reportar drift até a política estar definida |
| Stopped | Running | stop unit |
| Stopped | Stopped | no-op |
| Stopped | Missing | no-op |

`Missing` é uma observação, não um tombstone. `removed_at` é gravado somente no
cleanup de candidate, retirement do runtime anterior ou remoção intencional.

### 1.3 Exposure desired × observed

Definir o observado como: conteúdo canônico do fragmento no disco, reload
confirmado e health externo quando a rota é pública. Fragmento correto em disco
sem reload bem-sucedido não é exposição correta.

| Desired | Observed | Ação |
|---|---|---|
| Public | fragmento, reload e health corretos | no-op |
| Public | fragmento ausente | materialize e reload |
| Public | fragmento divergente | replace, reload e health |
| Internal | fragmento ausente | no-op |
| Internal | fragmento presente | remove e reload |

### 1.4 Deployment recovery e non-goals

- [ ] Definir política conservadora para interrupção em Pending, Starting,
      Verifying e Activating: observar, preservar o runtime anterior saudável,
      fazer cleanup seguro, registrar falha/interrupção; nunca promover uma
      situação ambígua automaticamente.
- [ ] Registrar explicitamente que reconcile não cria Release, não faz build,
      não descobre versão nova, não cria Deployment só porque um container
      morreu, não muda desired state, não promove candidate incerto e não faz
      mudança destrutiva diante de estado ambíguo.
- [ ] Registrar a interação futura `deploy × reconcile` antes de implementar o
      segundo, mas não construir lock adicional agora.

---

## Passo 2 — Aplicar o contrato de import remoto

- [ ] Em `src/main.rs`, rejeitar argumentos que não sejam URLs Git remotas;
      `file://` é aceito para repositórios de teste locais.
- [ ] Preservar o clone temporário e sua limpeza para toda importação CLI.
- [ ] Adicionar teste CLI para path local rejeitado sem criar Application.
- [ ] Converter `tests/cli.rs` e `tests/deployment_deploy_release.rs` que usam
      `pneuma app import <path>` para repositórios Git temporários `file://`.
- [ ] Manter testes diretos de `import_application` com paths de fixture: eles
      exercitam o use case sobre checkout já resolvido, não o contrato da CLI.
- [ ] Converter `scripts/dev-vm/deploy-all-fixtures.sh` para construir bare
      repositories locais e importar via `file://`, em vez de importar os
      diretórios de fixture diretamente.
- [ ] Atualizar documentação operacional e `verify-vps.sh` para o contrato
      remoto.

---

## Passo 3 — Consolidar tipos de domínio

- [ ] Criar `src/domain/deployment.rs` e mover de
      `src/use_cases/deployment_create.rs`:
      `Deployment`, `DeploymentType` e `DeploymentStatus`.
- [ ] Criar `src/domain/runtime.rs` e mover `RuntimeState` e
      `DesiredRuntimeState` de `application_runtime.rs`.
- [ ] Exportar os módulos em `src/domain/mod.rs`.
- [ ] `ObservedRuntimeState` permanece em `adapters/local_runtime.rs`: é a
      representação da observação do Podman, não estado desejado/persistido.
- [ ] Projeções de store como `RuntimeInstance` e `CandidateRuntime` podem
      continuar específicas ao adapter/use case; não criar um agregado Runtime
      adicional sem uso concreto.
- [ ] Decidir e documentar que enums de domínio fornecem serialização textual
      estável, enquanto o store é responsável por converter rows e rejeitar
      valores persistidos inválidos.
- [ ] Atualizar todos os imports de produção e testes; não alterar schema,
      máquina de estados ou saída CLI neste commit.

---

## Passo 4 — Mover provenance para Deployment

### 4.1 Migration 0014

- [ ] Criar `migrations/0014_deployment_source_revision.sql`:

```sql
ALTER TABLE deployments ADD COLUMN source_revision TEXT;

UPDATE deployments
SET source_revision = (
    SELECT releases.source_revision
    FROM releases
    WHERE releases.id = deployments.release_id
)
WHERE source_revision IS NULL;
```

- [ ] Registrar a migration em `src/adapters/database.rs` e atualizar as
      asserções da contagem de migrations.
- [ ] Testar upgrade a partir de um banco pré-0014 com deployment histórico,
      verificando o backfill.

### 4.2 Modelo e fluxos

- [ ] Remover `source_revision` de `domain::release::Release`; Release passa a
      ser artifact-only.
- [ ] Adicionar `source_revision: Option<String>` a `domain::deployment::Deployment`.
- [ ] Mudar `create_deployment` e `deployment_store::insert_pending_deployment`
      para receber/persistir a provenance da solicitação.
- [ ] `deploy_oci` passa a source revision recebida ao criar o Deployment, não à
      Release.
- [ ] `deploy_branch` persiste o SHA resolvido no Deployment.
- [ ] Rollback deve copiar a provenance do deployment histórico escolhido, não
      de uma Release compartilhada.
- [ ] `app deployments` lê `deployments.source_revision`.
- [ ] Remover o uso novo de `releases.source_revision`; a coluna legada pode
      permanecer no schema para compatibilidade dos bancos existentes.

### 4.3 Casos de teste

- [ ] OCI primeiro e branch depois para o mesmo digest: o segundo deployment
      guarda o SHA corretamente.
- [ ] Branch primeiro e OCI depois para o mesmo digest: o deployment OCI guarda
      `NULL` sem apagar o histórico Git anterior.
- [ ] Dois commits para o mesmo digest: cada deployment preserva sua provenance.
- [ ] Rollback mantém a provenance do deployment/Release histórico definido pela
      política de rollback.

---

## Passo 5 — Unificar Release e renomear resultado de deploy

- [ ] Em `src/adapters/stores/release_store.rs`, remover a struct local
      `Release` e retornar `domain::release::Release` diretamente.
- [ ] Em `release_create.rs`, remover a conversão manual campo a campo.
- [ ] Renomear `DeployedRelease` para `DeploymentResult` em
      `deployment_deploy_release.rs`, `deployment_deploy_oci.rs`,
      `deployment_deploy_branch.rs` e `deployment_rollback.rs`.
- [ ] Manter campos e output CLI existentes, trocando source revision para a
      provenance do Deployment recém-criado quando for exibida.

---

## Passo 6 — Separar artifact identity, labels e rota Caddy

### 6.1 Runtime materializado

- [ ] Renomear parâmetros internos atualmente chamados `source_revision` para
      `artifact_identity` quando alimentam Quadlet, candidate start ou runtime.
- [ ] Passar sempre `release.image_digest` para a materialização Quadlet.
- [ ] Escrever uma label explícita de artifact, preferencialmente
      `io.pneuma.image-digest=<digest>`.
- [ ] Decidir se `io.pneuma.revision` permanece como alias temporário com digest
      ou se é removida de novas unidades; em ambos os casos, nenhuma nova label
      pode usar `release.id` ou commit Git como identidade do artifact.
- [ ] Documentar que labels legadas com `release.id` são aceitas até redeploy.

### 6.2 Configuração Caddy

- [ ] Criar helper puro que gere o conteúdo canônico da rota a partir de domain
      e endpoint; `materialize_caddy_fragment` deve usá-lo.
- [ ] Persistir esse conteúdo canônico ou seu hash determinístico como
      `configuration_version`.
- [ ] O valor deve mudar se domain ou endpoint mudar, mesmo com o mesmo digest.
- [ ] `active_runtime_id` continua identificando a RuntimeInstance ativa; não
      duplicar esse papel em `configuration_version`.

### 6.3 Testes

- [ ] Nova materialização OCI usa digest, nunca `release.id`, em labels/Quadlet.
- [ ] Deploy por branch também materializa digest, mas seu Deployment preserva
      o SHA em `source_revision`.
- [ ] Alterar endpoint ou domain muda `configuration_version`.
- [ ] Banco/Quadlet legado com `release.id` continua operável até redeploy.

---

## Passo 7 — Extrair persistência de application_import

- [ ] Manter no use case: leitura/validação de manifest, resolução de system e
      fronteira transacional do agregado.
- [ ] Mover para `application_store` a geração/consulta de IDs, criação de
      system/application e inserção de delivery/source/runtime/health/exposure.
- [ ] Store deve receber `DeliveryType` e `Visibility`, não strings SQL.
- [ ] Adicionar `load_application_for_import(&Transaction, name) -> Option<Application>`.
      A query deve manter o `LEFT JOIN application_sources` e carregar também
      `active_deployment_id`; não pode devolver uma Application parcialmente
      preenchida após reimport de app deployada.
- [ ] Definir formalmente o reimport: nesta etapa é create-only/idempotente;
      specs, system, source e manifest divergentes não são atualizados
      silenciosamente. O CLI deve reportar o estado real da Application existente.
- [ ] Remover todo `query_row`, `execute` e `prepare` de `application_import.rs`.
- [ ] Adicionar testes para import remoto OCI sem source repetido, reimport de
      aplicação já deployada e falha de persistência no meio do agregado sem
      deixar specs parciais.

---

## Passo 8 — Extrair persistência de application_runtime

- [ ] Manter no use case: decisão desired×observed, ordem de efeitos externos e
      escolha de quando uma observação Missing é recuperável.
- [ ] Em `runtime_store`, definir uma projeção tipada
      `CurrentSuccessfulRuntime` contendo `runtime_id`, `external_runtime_id`,
      `deployment_id` e **`container_port`**.
- [ ] `load_current_successful_runtime` retorna `Option` e preserva todos os
      predicados atuais: deployment ativo, runtime running/stopped, não removido
      e deployment succeeded.
- [ ] Tornar a reconciliação de external ID compare-and-set:
      `id`, external ID esperado e `removed_at IS NULL` devem fazer parte do
      `WHERE`; zero rows resulta em `RuntimeChanged` antes de nova observação.
- [ ] Criar uma única API tipada de persistência de observação. Ela deve:
      atualizar `last_observed_at`; preservar `host_port` sem endpoint; atualizar
      porta com endpoint; retornar `updated == 1`; não tombar o runtime por uma
      observação Missing recuperável.
- [ ] Desired state deve ser carregado/persistido como `DesiredRuntimeState`,
      com parsing explícito de valor inválido e compare-and-set quando houver
      risco de mudança concorrente.
- [ ] Remover métodos de store duplicados ou mortos em vez de criar famílias
      paralelas de APIs.
- [ ] Remover todo SQL inline de `application_runtime.rs`.

Testes adicionais proporcionais:

- [ ] runtime de deployment não succeeded não é selecionado;
- [ ] falha de controle externo mantém desired state já persistido;
- [ ] compare-and-set perdido resulta em `RuntimeChanged`;
- [ ] observação running atualiza endpoint e timestamp.

Os casos Missing+Running, Missing+Stopped, stop/start e reconciliação de ID já
cobertos em `tests/cli.rs` devem ser mantidos, não duplicados.

---

## Passo 9 — Refatorar exposure_change para intenção primeiro

### 9.1 Persistência e CAS

- [ ] `application_store` deve carregar uma `StoredExposure` tipada com
      `Visibility` e `Option<String>` para domain; valor inválido no banco não
      pode ser silenciosamente convertido para Internal.
- [ ] Corrigir a leitura nullable de domain: `load_exposure_domain` atual infere
      `String` e falha para SQL NULL.
- [ ] Reutilizar/substituir os métodos existentes de update, em vez de criar
      variantes `_in_transaction`; eles devem receber `&Transaction` e retornar
      `bool` para indicar que exatamente uma linha foi atualizada.
- [ ] Persistir Public/Internal e `materialization_state` antes de Caddy:
      `applying` para Public e `removing` para Internal.
- [ ] Após sucesso externo, finalizar `active` ou `not_materialized`, atualizar
      runtime/configuration version quando aplicável e limpar diagnóstico.
- [ ] Após falha, manter desired visibility solicitada e gravar `failed` ou
      `diverged`, com diagnóstico.

### 9.2 Compensação

- [ ] Public: se materialização, health, update ou commit posterior falhar,
      tentar restaurar fragmento anterior e reload; falha de recuperação marca
      `diverged`.
- [ ] Internal: definir recuperação para falha após remoção do fragmento; se não
      for possível restaurar, registrar `diverged` explicitamente, sem alegar
      compensação completa.
- [ ] Zero rows após efeito externo nunca pode ser reportado como sucesso.
- [ ] Resolver o uso de `ExposureChangeError::InvalidDomain`: validar no use
      case ou remover a variante morta.
- [ ] Remover todo SQL inline de `exposure_change.rs`.

Testes adicionais proporcionais:

- [ ] Public sem runtime ativo;
- [ ] Public sem domain;
- [ ] falha de health restaura fragmento e mantém intenção Public com erro;
- [ ] falha de persistência ou CAS após materialização não é sucesso;
- [ ] falha de restauração marca `diverged`.

---

## Passo 10 — Finalizar deployment_create

- [ ] O use case mantém `TransactionBehavior::Immediate`, regras e commit.
- [ ] `deployment_store` recebe métodos transacionais para:
      `has_nonterminal_deployment`, `load_active_runtime_release_id`,
      `insert_pending_deployment` e `load_deployment`.
- [ ] Todos aceitam `&Transaction`; `generate_id` também pode ser transacional.
- [ ] `load_deployment` deve selecionar e validar type, status e
      `source_revision`; não pode assumir Pending nem converter tipo inválido
      silenciosamente para Deploy.
- [ ] Preservar predicados de AlreadyActive: deployment ativo da application,
      runtime running/stopped e não removido. Rollback da mesma Release continua
      permitido.
- [ ] Remover conversões genéricas de `StoreError` que transformam deployment
      ausente em ReleaseNotFound, estado inválido em QueryReturnedNoRows ou
      SystemNotFound em application `unknown`.
- [ ] Remover SQL inline de `deployment_create.rs`.

Testes:

- [ ] runtime ativo running bloqueia Deploy da mesma Release;
- [ ] runtime ativo stopped bloqueia Deploy;
- [ ] runtime removido não aciona AlreadyActive;
- [ ] Rollback da Release atual é permitido;
- [ ] aplicação existente com Release ausente retorna ReleaseNotFound.

---

## Passo 11 — Provar exclusão de deployment

São necessárias duas provas distintas.

### 11.1 Lock Immediate

- [ ] Em `tests/deployment_create.rs`, usar banco temporário em arquivo e duas
      conexões abertas antes do lock.
- [ ] A primeira segura uma transaction `Immediate`; a segunda usa busy timeout
      zero e chama `create_deployment` para uma aplicação ausente.
- [ ] Esperado: `CreateDeploymentError::Persistence` com
      `rusqlite::ErrorCode::DatabaseBusy`.
- [ ] Isso falha se `Immediate` virar `Deferred`, pois a segunda conexão então
      conseguiria ler e retornaria ApplicationNotFound.

### 11.2 Reserva lógica cross-process

- [ ] Em `tests/cli.rs`, iniciar deploy A como processo e bloquear somente o
      fake systemctl no start, depois de A persistir deployment non-terminal.
- [ ] O gate usa marker/release files configurados por ambiente, timeout no pai e
      cleanup garantido do filho.
- [ ] Processo B, sem gate, deve falhar com `ActiveDeployment`, nunca com
      `database is locked` ou erro de índice único.
- [ ] Liberar A e confirmar que finaliza succeeded com um deployment e um
      runtime running.

O primeiro teste prova aquisição antecipada do writer lock; o segundo prova que
o deployment persistido reserva a Application durante efeitos externos.

---

## Passo 12 — Cenários E2E da v0.3

Criar `docs/design/reconciliation-e2e.md`, indexado no `docs/README.md`. O
arquivo define cenários; eles só entram em `scripts/dev-vm/test-all.sh` quando
`pneuma reconcile` existir.

### Runtime drift

- [ ] container removido antes e depois de status;
- [ ] unit presente com container ausente;
- [ ] unit ausente;
- [ ] runtime Running com image digest, labels, porta ou unit divergentes;
- [ ] reboot.

### Exposure drift

- [ ] fragmento removido;
- [ ] fragmento com target errado;
- [ ] fragmento correto no disco, mas reload não confirmado;
- [ ] desired Public sem rota;
- [ ] desired Internal com rota.

### Deployment recovery

- [ ] interrupção em Pending, Starting, Verifying e Activating.

### Concorrência e idempotência

- [ ] reconcile duas vezes;
- [ ] reconcile paralelo;
- [ ] deploy × deploy;
- [ ] deploy × reconcile.

---

## Passo 13 — Regressão e encerramento

- [ ] Rodar os quatro gates após cada refactor funcional e novamente ao final.
- [ ] Validar migration 0014 em banco novo e em banco pré-migration.
- [ ] Em VM Debian 13 limpa, executar: bootstrap → doctor → system create →
      import Git remoto → deploy por branch → candidate → health → promotion →
      status → stop/start → visibility → rollback → reboot → CI dispatcher.
- [ ] Rodar `scripts/dev-vm/test-all.sh` e registrar no tracker a contagem real
      de passes, falhas e skips, sem congelar uma baseline antiga no plano.
- [ ] Atualizar `docs/architecture/architecture.md`, `README.md` e documentos
      operacionais para refletir o comportamento realmente entregue.
- [ ] Só depois de tudo verde, marcar a iteração concluída e fazer um commit
      `docs:` contendo apenas `docs/iterations/current-iteration.md`.

---

## Fora de escopo

Não bloquear v0.3 por registry watcher, auto-deploy policies, idempotency key
genérica, audit trail completo, image retention, API HTTP, TUI, OIDC, GitHub
App, RBAC ou novo usuário Linux. Também ficam fora desta consolidação a
implementação de `pneuma reconcile` e a reparação automática de materializações
v0.2 legadas.

## Definition of Done

- [ ] CLI aceita apenas Git URLs e todos os scripts/testes públicos usam esse
      contrato.
- [ ] Release é artifact-only; Deployment registra source revision por tentativa.
- [ ] Migration 0014 e seu upgrade test estão verdes.
- [ ] Novas unidades usam digest como identidade do artifact; legados estão
      documentados como compatíveis até redeploy.
- [ ] `configuration_version` identifica a rota Caddy materializada.
- [ ] `RuntimeState` e `DesiredRuntimeState` são tipos de domínio; observação
      Podman continua no adapter.
- [ ] Os quatro use cases alvo não contêm SQL inline.
- [ ] Updates críticos usam CAS/row-count e erros de store são mapeados sem
      valores inventados.
- [ ] Intent de visibility é persistida antes de Caddy e falhas registram estado
      materializado recuperável.
- [ ] O teste de Immediate falha se a transaction se tornar Deferred.
- [ ] O teste cross-process produz ActiveDeployment durante deployment em curso.
- [ ] O design de reconciliation cobre identidade, Missing sem tombstone,
      exposição, deployments interrompidos e non-goals.
- [ ] O catálogo E2E da v0.3 está documentado e indexado.
- [ ] Quatro gates e regressão de VM estão verdes.
