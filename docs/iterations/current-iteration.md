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
- [ ] Aplicar o contrato de importação exclusivamente por URL Git.
- [ ] Consolidar os tipos de domínio de Deployment e Runtime.
- [ ] Migrar `source_revision` para Deployment com cobertura fresh/upgrade.
- [ ] Unificar Release artifact-only e renomear `DeploymentResult`.
- [ ] Materializar image digest como identidade de runtime e versão de rota.
- [ ] Extrair persistência de `application_import` para store.
- [ ] Extrair persistência de `application_runtime` para stores.
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
