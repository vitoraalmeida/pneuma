# Iteração atual

**Status:** concluída em 12 de agosto de 2026

**Base:** `99fd151` (`docs: revise pre-v0.3 consolidation decisions`)

**Design aprovado:** [`design/pre-v0.3-consolidation.md`](../design/pre-v0.3-consolidation.md)

## Iteração — Pré-v0.3: consolidação de domínio e persistência

Objetivo: consolidar o modelo Release/Deployment/Runtime, a persistência e a
semântica de reconciliation antes de implementar `pneuma reconcile`.

### Escopo e non-goals

- Release é artifact OCI imutável; `Deployment` registra proveniência Git por
  tentativa; runtime materializa sempre o image digest.
- A consolidação não implementa `pneuma reconcile`, registry watcher,
  auto-deploy, OIDC, API HTTP, TUI, RBAC ou novo usuário Linux.
- A execução segue os checkpoints do design na ordem abaixo. O primeiro item
  desmarcado é o próximo trabalho autorizado.

## Checkpoints

- [x] Abrir a iteração e registrar o design aprovado.
- [x] Redefinir roadmap e sincronizar documentação pública pós-v0.2.
      Resultado: roadmap reescrito para v0.1→v0.7 (v0.3 = reconciliation/reliability),
      CI/CD já entregue documentado como concluído, import público por URL Git nos
      docs e scripts (deploy-all-fixtures.sh via `file://`, verify-vps.sh) e referência
      quebrada a `docs/operations/e2e-testing.md` removida.
- [x] Definir semântica, invariantes e fontes de verdade de reconciliation.
      Resultado: design indexado define autoridades, invariantes, matrizes de
      runtime/exposure, recovery conservador de deployments interrompidos,
      concorrência e non-goals sem implementar `pneuma reconcile`.
- [x] Aplicar o contrato de importação exclusivamente por URL Git.
      Resultado: CLI rejeita paths antes de efeitos, clona toda URL aceita em
      checkout temporário (inclusive limpeza após falha de clone) e os testes
      binários importam repositórios temporários via `file://`.
- [x] Consolidar os tipos de domínio de Deployment e Runtime.
      Resultado: Deployment, DeploymentType, DeploymentStatus, RuntimeState e
      DesiredRuntimeState vivem em módulos de domínio; ObservedRuntimeState
      permanece no adapter, sem alterar schema, transições ou saída CLI.
- [x] Migrar `source_revision` para Deployment com cobertura fresh/upgrade.
      Resultado: migration 0014 faz backfill histórico; Release novo é
      artifact-only e cada Deployment persiste provenance própria, inclusive
      deploy por branch, OCI do mesmo digest e rollback.
- [x] Unificar Release artifact-only e renomear `DeploymentResult`.
      Resultado: store retorna o único Release de domínio e o resultado dos
      caminhos de deploy chama-se DeploymentResult, preservando campos e saída
      CLI existentes.
- [x] Materializar image digest como identidade de runtime e versão de rota.
      Resultado: novas unidades usam `io.pneuma.image-digest`; a versão da rota
      é o fragmento Caddy canônico (domain + endpoint), e materializações v0.2
      legadas permanecem operáveis até redeploy.
- [x] Extrair persistência de `application_import` para store.
      Resultado: SQL inline removido de `application_import.rs`; store ganha
      `load_application_for_import` com `active_deployment_id` real e recebe
      `DeliveryType`/`Visibility` tipados; reimport é create-only, atômico e o
      CLI reporta o estado real de Applications já deployadas.
- [x] Extrair persistência de `application_runtime` para stores.
      Resultado: `runtime_store` carrega runtime/desired state tipados e aplica
      CAS a `external_runtime_id` e desired state; observações atualizam
      timestamp/endpoint sem tombar `Missing`, APIs mortas foram removidas e
      lifecycle continua recuperando a RuntimeInstance pela unidade Quadlet.
- [x] Persistir desired visibility antes de materializar Caddy.
      Resultado: visibility e estado `applying`/`removing` são persistidos por
      CAS antes de Caddy; conclusões/falhas são recuperáveis com diagnóstico,
      e materialização ou remoção compensam o fragmento quando a persistência
      posterior falha.
- [x] Finalizar extração de persistência de `deployment_create`.
      Resultado: `deployment_store` encapsula consultas, insert e leitura
      validada de Deployment sob `Immediate`; `deployment_create.rs` mantém
      regras/commit sem SQL inline e testes cobrem runtime ativo/removido,
      rollback e Release ausente.
- [x] Provar lock Immediate e reserva lógica cross-process de deployment.
      Resultado: duas conexões provam aquisição antecipada de writer lock com
      `DatabaseBusy`; dois processos CLI provam que deployment não-terminal
      retorna `ActiveDeployment` durante efeitos externos, sem lock ou índice.
- [x] Definir catálogo de cenários E2E de reconciliation da v0.3.
      Resultado: catálogo indexado cobre drift de runtime/exposure, recovery de
      deployments interrompidos e concorrência; automação VM permanece adiada
      até `pneuma reconcile` existir.
- [x] Preparar VM Debian 13 reproduzível e executar regressão final de código,
      migration e VM.
      Plano: a partir de clone descartável de `pneuma-dev-base`, gerar localmente
      `~/.ssh/pneuma-ci-test`; usar bootstrap nativo na VM a partir da URL pública
      do repositório, fixado em `b6887a4`, para instalar o binário e somente a
      chave pública CI restrita; validar bootstrap e rerun; criar o snapshot
      `pneuma-ready` apenas depois da aceitação; executar `test-all.sh` em clone
      descartável de `pneuma-ready`, com a chave privada local, e registrar a
      contagem real PASS/FAIL/SKIP. O bootstrap da VM pode usar autenticação root
      por senha somente no wrapper local, via `PNEUMA_VM_ROOT_PASSWORD`; nenhum
      segredo entra em script, argumento, log ou VM.
      Resultado: bootstrap nativo a partir de `b6887a4` e rerun passaram em clone
      Debian 13; snapshot `pneuma-ready` criado. A bateria no clone descartável
      passou com 27 PASS / 0 FAIL / 1 SKIP (`redirect-public` requer a configuração
      opcional de `local_certs`); os scripts VM usam SSH root diretamente para
      operações administrativas e executam runtime como `pneuma`.

## Critérios de aceite

- [x] O roadmap, a arquitetura, o README e a documentação operacional refletem
  o comportamento entregue.
- [x] A CLI aceita somente URLs Git; testes e scripts públicos usam esse
  contrato.
- [x] Release é artifact-only e cada Deployment preserva `source_revision`.
- [x] A migration de provenance passa em banco novo e upgrade histórico.
- [x] Novas unidades materializam image digest; materializações v0.2 legadas são
  compatíveis até redeploy.
- [x] `configuration_version` identifica a rota Caddy materializada.
- [x] `RuntimeState` e `DesiredRuntimeState` pertencem ao domínio; observação
  Podman permanece no adapter.
- [x] `application_import`, `application_runtime`, `exposure_change` e
  `deployment_create` não contêm SQL inline.
- [x] Updates críticos usam CAS/row count e erros de store não inventam IDs.
- [x] Desired visibility é persistida antes de Caddy e falhas ficam recuperáveis.
- [x] Os testes provam lock Immediate e rejeição cross-process por
  `ActiveDeployment`.
- [x] Designs de reconciliation e seus cenários E2E estão indexados.
- [x] Os quatro gates, migration coverage e regressão VM exigida estão verdes.

## Bloqueadores

Nenhum.

## Validação final

Os quatro gates passaram após o ajuste final de scripts: `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo test
--all-features` e `cargo build --workspace --release`. A suite cobriu banco novo
e upgrade 0014 (`open_configures_and_migrates_database` e
`upgrades_release_provenance_to_historical_deployments`); a regressão VM passou
com 27 PASS / 0 FAIL / 1 SKIP documentado.

## Próxima iteração proposta

Após encerrar esta iteração, abrir design aprovado e tracker separado para
fortalecer bootstrap e E2E conforme
`~/Downloads/pneuma-bootstrap-vm-e2e-hardening-plan.md`. O escopo proposto é:

- extrair invariantes compartilhados de provisionamento entre bootstrap VPS e VM;
- tornar preflight, usuário, subuid/subgid, Caddy e ambiente idempotentes e
  verificáveis;
- adicionar `--ref`, configuração Caddy atômica e testes de bootstrap limpo com
  segunda execução;
- tornar E2E rigoroso para candidate falho, rollback real, reboot, HTTPS público,
  segurança do dispatcher CI e semântica de backup/restore;
- incluir lint e testes de scripts shell no CI e atualizar documentação
  operacional.

Esse escopo não está autorizado neste checkpoint, exceto pelas mudanças mínimas
necessárias para criar uma VM Debian 13 `pneuma-ready` e executar a regressão
final acima.
