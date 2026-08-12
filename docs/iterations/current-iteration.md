# Iteração atual

**Status:** em andamento

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
- [ ] Persistir desired visibility antes de materializar Caddy.
- [ ] Finalizar extração de persistência de `deployment_create`.
- [ ] Provar lock Immediate e reserva lógica cross-process de deployment.
- [ ] Definir catálogo de cenários E2E de reconciliation da v0.3.
- [ ] Executar regressão final de código, migration e VM.

## Critérios de aceite

- [ ] O roadmap, a arquitetura, o README e a documentação operacional refletem
  o comportamento entregue.
- [ ] A CLI aceita somente URLs Git; testes e scripts públicos usam esse
  contrato.
- [ ] Release é artifact-only e cada Deployment preserva `source_revision`.
- [ ] A migration de provenance passa em banco novo e upgrade histórico.
- [ ] Novas unidades materializam image digest; materializações v0.2 legadas são
  compatíveis até redeploy.
- [ ] `configuration_version` identifica a rota Caddy materializada.
- [ ] `RuntimeState` e `DesiredRuntimeState` pertencem ao domínio; observação
  Podman permanece no adapter.
- [ ] `application_import`, `application_runtime`, `exposure_change` e
  `deployment_create` não contêm SQL inline.
- [ ] Updates críticos usam CAS/row count e erros de store não inventam IDs.
- [ ] Desired visibility é persistida antes de Caddy e falhas ficam recuperáveis.
- [ ] Os testes provam lock Immediate e rejeição cross-process por
  `ActiveDeployment`.
- [ ] Designs de reconciliation e seus cenários E2E estão indexados.
- [ ] Os quatro gates, migration coverage e regressão VM exigida estão verdes.

## Bloqueadores

Nenhum.

## Validação final

Pendente: quatro gates no commit final de código, banco novo e upgrade da
migration, e regressão em VM Debian 13 conforme os critérios de aceite.
