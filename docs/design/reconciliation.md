# Design — Reconciliation

**Status:** design aprovado para a v0.3; não descreve comportamento já
implementado. A execução e o progresso vivem somente em
`docs/iterations/current-iteration.md`.

## Objetivo

Definir a semântica que o futuro `pneuma reconcile` usará para convergir uma
Application para a intenção persistida, sem escolher uma versão nova nem
transformar uma observação ambígua em alteração destrutiva. Este design orienta
os refactors de domínio, runtime e exposure da consolidação pré-v0.3.

Este documento não introduz o comando, APIs, migrations ou políticas de retry.

## Modelo e vocabulário

- **Intenção:** estado desejado persistido no SQLite, incluindo runtime e
  visibility.
- **Estado lógico:** Application, Release, Deployment e RuntimeInstance
  persistidos; seus IDs não são substituídos por IDs externos.
- **Estado observado:** container e unidade Quadlet observados em
  Podman/systemd, e rota observada no Caddy.
- **Materialização correta:** estado observado que corresponde integralmente à
  intenção e à identidade lógica esperada.
- **Drift:** divergência entre a materialização observada e a configuração ou
  identidade esperada.
- **Retirement:** remoção intencional de um runtime durante cleanup ou troca de
  runtime ativo. Somente retirement grava `removed_at`.

Release é o artifact OCI imutável. Deployment é a tentativa de ativar uma
Release. RuntimeInstance é a materialização concreta de um Deployment e é
reutilizada durante a recuperação de drift recuperável.

## Fontes de verdade

| Sistema | Autoridade |
|---|---|
| SQLite | Intenção, histórico e identidades lógicas. |
| Podman/systemd | Estado observado do container e da unidade Quadlet. |
| Caddy | Fragmento, reload e rota materializados. |
| OCI registry | Disponibilidade de artifact, nunca seleção de versão pelo reconcile. |

O reconciler não consulta o registry para criar ou selecionar uma Release. Uma
Application com container ausente não recebe Deployment novo por esse motivo.

## Invariantes

1. O deployment ativo e seu runtime ativo definem a identidade lógica que pode
   ser recuperada. Recovery não cria Deployment ou RuntimeInstance novos.
2. `Missing` é uma observação, não um tombstone. `removed_at` é reservado a
   cleanup de candidate, retirement de runtime anterior e remoção intencional.
3. `Running/Running` somente é no-op quando deployment ativo, image digest,
   labels, endpoint loopback, porta, unidade Quadlet e container correspondem
   à materialização esperada.
4. Nenhuma transação SQLite permanece aberta durante Podman, systemd, Caddy ou
   HTTP. O use case observa ou materializa externamente e persiste o resultado
   em transação curta.
5. Toda persistência posterior a efeito externo usa compare-and-set e row
   count. Zero linhas é estado stale ou concorrente, nunca sucesso.
6. Materializações v0.2 que usam `release.id` em labels, Quadlet ou
   `configuration_version` continuam observáveis até redeploy ou
   rematerialização. Novas materializações usam as regras de identidade
   definidas nos refactors correspondentes.
7. Diante de identidade ambígua ou configuração divergente, reconcile não para,
   remove, substitui nem promove recursos automaticamente sem política
   explícita.

## Reconciliação de runtime

Antes de agir, reconcile carrega a intenção, o deployment ativo e a
RuntimeInstance bem-sucedida não removida. A observação confirma o nome
determinístico do container/unidade, a relação com a Application e o
Deployment, image digest, labels, porta loopback e conteúdo/configuração da
unidade Quadlet. A atualização de `external_runtime_id` usa o runtime lógico
esperado e falha como `RuntimeChanged` se o CAS não atualizar exatamente uma
linha.

| Desired | Observed | Ação |
|---|---|---|
| Running | Running, identidade e configuração corretas | No-op. |
| Running | Stopped, unidade presente e identidade confirmada | Iniciar a unidade e observar novamente. |
| Running | Missing, unidade presente e identidade confirmada | Iniciar a unidade, resolver o container pelo nome estável e reconciliar `external_runtime_id`. |
| Running | Missing, unidade ausente e identidade lógica confirmada | Rematerializar a mesma unidade para a mesma RuntimeInstance, iniciar e reconciliar `external_runtime_id`. |
| Running | Failed | Coletar diagnóstico seguro; reiniciar somente quando unidade e identidade forem confirmadas e a política de recuperação o permitir. Caso contrário, reportar sem alteração destrutiva. |
| Running | Running com digest, label, porta, container ou unidade divergente | Reportar drift e requerer política explícita ou intervenção manual. |
| Stopped | Running, Starting, Created ou Stopping | Parar a unidade quando ela for a unidade esperada; observar novamente. |
| Stopped | Stopped | No-op. |
| Stopped | Missing | No-op; manter RuntimeInstance e não gravar `removed_at`. |
| Stopped | Failed ou unidade divergente | Reportar diagnóstico; não remover a unidade ou tombar o runtime. |

Na rematerialização, a porta persistida da RuntimeInstance é a identidade de
endpoint esperada. Se ela não puder ser usada com segurança, reconcile reporta
drift em vez de alocar outra porta silenciosamente. O runtime legado permanece
operável pela compatibilidade v0.2 até uma rematerialização que aplique a nova
identidade.

## Reconciliação de exposure

Visibility é intenção persistida. A exposição pública está correta somente se o
fragmento canônico esperado estiver no disco, o reload tiver sido confirmado e
o health externo da rota tiver passado. Fragmento correto sem reload confirmado
ou sem health externo não é uma rota correta.

`configuration_version` identifica a representação canônica do fragmento Caddy
(`domain` e endpoint). Ele não identifica Release, Deployment ou RuntimeInstance.

| Desired | Observed | Ação |
|---|---|---|
| Public | Fragmento canônico, reload confirmado e health externo correto | No-op. |
| Public | Fragmento ausente, runtime ativo saudável e endpoint confirmado | Materializar, reload e executar health externo. |
| Public | Fragmento divergente, runtime ativo saudável e endpoint confirmado | Substituir pelo fragmento canônico, reload e executar health externo. |
| Public | Sem runtime ativo saudável, domain ou endpoint confirmado | Registrar diagnóstico recuperável; não publicar rota. |
| Internal | Fragmento ausente | No-op. |
| Internal | Fragmento presente | Remover, reload e confirmar remoção. |

Falha durante materialização, reload ou health preserva a intenção solicitada e
grava `failed` com diagnóstico. Se a compensação não puder restaurar uma
situação observável conhecida, grava `diverged`. Reconcile pode reparar
`failed` quando o fragmento esperado e a identidade do runtime forem
inequívocos; `diverged` é reportado para intervenção manual até que uma política
explícita defina como substituir um fragmento de origem ambígua.

## Recovery de deployments interrompidos

Deployment em `Pending`, `Starting`, `Verifying` ou `Activating` reserva a
Application e impede reconcile de competir por seus efeitos. O recovery futuro
observa primeiro candidate, unidade, rota e runtime anterior, preservando a
versão saudável já ativa.

| Status interrompido | Regra de recovery |
|---|---|
| Pending | Sem efeitos externos confirmados: registrar falha/interrupção e liberar apenas recursos comprovadamente associados ao deployment. |
| Starting | Observar candidate e unidade; limpar somente o candidate comprovadamente associado, registrar falha/interrupção e preservar runtime anterior. |
| Verifying | Não promover candidate cuja saúde não esteja comprovada; limpar candidate com identidade confirmada, registrar falha/interrupção e preservar runtime/rota anteriores. |
| Activating | Não inferir promoção a partir de fragmento, reload ou runtime isolados. Se a promoção atômica não estiver comprovada, restaurar apenas o que tiver compensação segura, registrar falha/interrupção e marcar exposure `diverged` quando a recuperação for incompleta. |

Recovery nunca promove automaticamente um candidate ambíguo. Falhas de cleanup
são diagnósticos recuperáveis e não revogam a promoção que já tenha sido
confirmada atomicamente.

## Concorrência e resultados

Não será criado lock adicional neste design. O deploy existente mantém a
reserva lógica por Deployment não terminal. Enquanto houver Deployment não
terminal, reconcile retorna resultado `deferred` com o deployment que bloqueia
a operação e não aciona runtime, Caddy ou cleanup concorrente.

O futuro comando deve serializar `reconcile × reconcile` por Application pela
mesma reserva ou por uma primitive persistida equivalente antes de executar
efeitos externos. Um CAS perdido após um efeito externo resulta em `failed` ou
`deferred`, com diagnóstico e sem reportar sucesso.

Resultados observáveis são:

- `no-op`: a materialização já era correta;
- `repaired`: uma divergência recuperável foi convergida e confirmada;
- `deferred`: deployment ou mudança concorrente impede ação segura;
- `failed`: a convergência não foi concluída, com diagnóstico recuperável;
- `diverged`: a compensação ou observação não permite afirmar a materialização;
- `manual-intervention`: drift de identidade ou configuração exige política
  explícita.

## Non-goals

Reconcile não cria Release, não faz build, não descobre artifact novo, não
seleciona versão no registry, não cria Deployment porque um container morreu,
não muda desired runtime state ou visibility, não promove candidate incerto e
não executa reparo destrutivo diante de identidade ambígua.

## Cenários de aceite futuros

O catálogo E2E posterior deve cobrir, no mínimo:

- container removido com unidade presente e com unidade ausente;
- digest, label, porta, container ou unidade divergentes;
- reboot e recovery de runtime stopped/running;
- fragmento Caddy ausente ou com target divergente, reload não confirmado e
  desired visibility incompatível com a rota;
- interrupção em cada status não terminal de Deployment;
- reconcile repetido, reconcile paralelo, deploy × deploy e deploy × reconcile.
