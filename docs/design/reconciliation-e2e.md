# Catálogo E2E — Reconciliation v0.3

**Status:** catálogo aprovado para implementação futura; não descreve testes já
executados nem introduz `pneuma reconcile`.

**Semântica:** [`reconciliation.md`](reconciliation.md) define as autoridades,
invariantes, resultados e non-goals. Este catálogo define a prova operacional
em VM Debian 13 depois que o comando existir.

## Limites

- Estes cenários não entram em `scripts/dev-vm/test-all.sh` antes de
  `pneuma reconcile` existir.
- O catálogo não autoriza criar Release, construir imagem, consultar registry
  para escolher versão, criar Deployment por drift, alterar intenção ou fazer
  reparo destrutivo diante de identidade ambígua.
- Cada cenário observa SQLite, Podman/systemd e Caddy conforme aplicável. Um
  resultado de processo bem-sucedido sem estado materializado confirmado não é
  sucesso do cenário.

## Ambiente Futuro

Os testes devem usar uma VM Debian 13 limpa, com Podman rootless, linger do
usuário `pneuma`, systemd user/Quadlet, Caddy, banco SQLite e imagens fixture
disponíveis. O harness precisa conseguir:

- importar por URL Git e fazer deploy de aplicação interna e pública;
- inspecionar `runtime_instances`, `deployments` e `exposures` no SQLite;
- remover/inspecionar containers e unidades Quadlet sem alterar identidades;
- editar/remover fragmentos Caddy e confirmar reload/health externo;
- interromper o processo Pneuma em pontos controlados dos estados não terminais;
- executar comandos com timeout, coletar logs e limpar recursos no fim.

Cada caso deve registrar a versão de fixture, IDs de Application/Deployment/
RuntimeInstance, estado desejado inicial e resultado final. Os resultados
esperados usam: `no-op`, `repaired`, `deferred`, `failed`, `diverged` ou
`manual-intervention`.

## Runtime Drift

| Cenário | Injeção e ação | Resultado esperado | Proibido |
|---|---|---|---|
| Container removido antes de observação | Remover o container do deployment ativo com intent `Running`; executar reconcile. | Com unidade presente e identidade confirmada, `repaired`: a mesma RuntimeInstance/Deployment é reutilizada, porta persistida é mantida e `external_runtime_id` é reconciliado por CAS. | Criar Deployment/RuntimeInstance, tombar `removed_at`, trocar porta. |
| Container removido depois de `status` | Executar `app status` para registrar `Missing`, remover/confirmar ausência e executar reconcile. | Mesmo recovery do caso anterior; `Missing` não impede recuperar a identidade lógica. | Interpretar `Missing` como retirement. |
| Unidade presente, container ausente | Manter Quadlet esperado e remover somente o container. | Iniciar unidade, observar container pelo nome estável e confirmar identidade, endpoint e CAS. | Reescrever unidade sem necessidade. |
| Unidade ausente, container ausente | Remover o Quadlet esperado e o container, preservando SQLite. | Rematerializar a mesma unidade para a mesma RuntimeInstance e iniciar somente após confirmar identidade lógica. | Criar novo Deployment, alocar porta nova ou promover candidate. |
| Identidade divergente em runtime Running | Alterar separadamente digest, label, porta, container ou conteúdo/unidade Quadlet. | `manual-intervention` com diagnóstico do campo divergente; nenhum efeito destrutivo. | Stop/remove/substituição automática. |
| Reboot com intent Running | Reboot após materialização correta. | Runtime ativo volta e reconcile produz `no-op` ou `repaired` apenas para ID externo renovado. | Criar Deployment novo. |
| Reboot com intent Stopped | Parar aplicação, rebootar e executar reconcile. | `no-op`; RuntimeInstance permanece sem `removed_at`, container ausente é aceitável. | Iniciar unidade ou tombar runtime. |

## Exposure Drift

| Cenário | Injeção e ação | Resultado esperado | Proibido |
|---|---|---|---|
| Fragmento público removido | Apagar fragmento canônico de Application `Public` saudável. | `repaired`: recriar fragmento, validar, reload e health externo confirmados; estado `active`. | Alterar visibility ou Deployment. |
| Upstream divergente | Alterar target loopback no fragmento público. | Substituir pelo conteúdo canônico, reload e health externo; atualizar `configuration_version` somente após confirmação. | Preservar fragmento divergente como sucesso. |
| Fragmento correto sem reload confirmado | Deixar conteúdo correto no disco e forçar reload a falhar. | Intent `Public` preservado; `failed` ou `diverged` com diagnóstico conforme compensação; não reportar rota ativa. | Declarar `no-op` só pelo conteúdo no disco. |
| Intent Public sem rota | Persistir `Public` com runtime saudável e fragmento ausente. | Materializar e confirmar rota, ou `failed` recuperável se pré-condição/efeito falhar. | Reverter intent para `Internal`. |
| Intent Internal com rota | Persistir `Internal` e manter fragmento no disco. | Remover fragmento, reload e confirmar ausência; estado `not_materialized`. | Manter rota pública ou alterar runtime. |
| Compensação de exposure falha | Forçar falha após materialização/remoção e falha na restauração. | `diverged` com diagnóstico explícito e estado observável preservado para intervenção. | Afirmar compensação completa. |

## Deployments Interrompidos

Todos os casos preservam runtime e rota anteriores saudáveis. O harness
interrompe o processo depois da transição persistida e antes do próximo efeito,
então executa reconcile uma vez.

| Estado | Evidência mínima e resultado esperado |
|---|---|
| `Pending` | Nenhum efeito externo confirmado. Registrar falha/interrupção e liberar somente reserva comprovadamente associada. |
| `Starting` | Observar candidate, unidade e container. Limpar somente recursos com identidade confirmada; registrar falha/interrupção. |
| `Verifying` | Não promover candidate sem health comprovado. Limpar candidate confirmado e preservar runtime/rota anteriores. |
| `Activating` | Não inferir promoção por fragmento, reload ou runtime isolado. Restaurar somente o que tiver compensação segura; registrar `diverged` se não for possível afirmar estado conhecido. |

Em todos: reconcile não promove candidate ambíguo, não cria Release e não muda
intent de runtime ou visibility.

## Concorrência E Idempotência

| Cenário | Execução | Resultado esperado |
|---|---|---|
| Reconcile repetido | Executar reconcile duas vezes sobre materialização correta e, separadamente, sobre drift recuperável. | Segundo resultado é `no-op` após convergência; não duplica unidades, rotas, RuntimeInstances ou Deployments. |
| Reconcile paralelo | Iniciar dois reconciles para a mesma Application, bloqueando o primeiro após adquirir reserva. | Um converte ou termina; o outro retorna `deferred`, sem `database is locked` e sem efeitos concorrentes. |
| Deploy x deploy | Bloquear deploy A depois de persistir Deployment não terminal e iniciar B. | B retorna `ActiveDeployment`; A pode concluir com um Deployment succeeded e um runtime running. A prova atual de CLI deve permanecer como regressão. |
| Deploy x reconcile | Bloquear deploy após persistir estado não terminal e executar reconcile. | Reconcile retorna `deferred`, não toca candidate, runtime anterior, Caddy ou cleanup. |

## Automação Posterior

Quando `pneuma reconcile` existir, cada linha deste catálogo vira um caso
nomeado no harness de VM. O script deve reportar PASS/FAIL/SKIP, guardar logs e
nunca marcar como PASS uma dependência ausente de registry, rede, VM ou
credencial. Skips exigem motivo explícito no tracker da iteração.
