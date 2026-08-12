Antes de começar a implementar a v0.3, eu faria uma etapa formal de consolidação da v0.2. Não é para remodelar o Pneuma; é para deixar o modelo que já existe explícito, consistente e suficientemente testado para o reconciler não nascer sobre ambiguidades.

O estado atual já é uma boa base: v0.2 está marcada como concluída, CI → OCI → SSH → ci dispatch → deploy funciona, o bootstrap foi validado em Debian 13 limpo, e a bateria atual registra 27 PASS / 0 FAIL / 1 SKIP.

O objetivo do pré-v0.3

Ao terminar essa etapa, estas afirmações precisam ser verdadeiras:

Release
= artifact imutável que pode ser implantado

Deployment
= tentativa/evento de ativar uma Release

Runtime
= materialização concreta produzida por um Deployment

Application
= possui desired state e aponta para o Deployment ativo

SQLite
= intenção + histórico

Podman/systemd/Caddy
= estado observado

Use case
= decisão/orquestração

Store
= persistência

CI
= apenas solicita operações ao Pneuma

Reconciler
= ainda não existe, mas suas regras já estão definidas

Hoje o modelo geral já aponta nessa direção: o próprio roadmap define Release como imutável, Deployment como tentativa e diferencia estado desejado de observado.

1. Corrigir o roadmap antes de qualquer código da v0.3

Esse seria o primeiro trabalho.

O roadmap.md atual ainda chama a v0.3 de “Reconciliation, automation & CI/CD” e apresenta como pendentes GitHub Actions, build/push para GHCR, deploy SSH e forced command. Isso já não representa o estado atual.

Eu mudaria para:

v0.1 — OCI Deployment Foundation         ✅
v0.2 — Git-aware OCI Delivery            ✅

v0.3 — Reconciliation & Deployment Reliability
v0.4 — Application Topology & Internal Networking
v0.5 — Network Policy Enforcement
v0.6 — Workload Identity & Secure S2S
v0.7 — Artifact Security & Secrets

E retiraria da v0.3 como trabalho futuro:

GitHub Actions build/push
SSH deployment
forced command
CI dispatcher
registry watcher
automatic deploy policies
dedicated pneuma-deployer

Os quatro primeiros já existem; registry watcher e auto-deploy policies não têm necessidade demonstrada no modelo atual; e a identidade restrita já foi implementada usando o próprio usuário pneuma.

Também reformularia:

rollback automático em health failure

porque candidate failing antes da promoção é diferente de fazer rollback automático depois de uma promoção.

2. Fechar formalmente a v0.2 e abrir uma iteração “pre-v0.3 consolidation”

Eu não começaria a próxima current-iteration.md com:

Implement Reconciler.

Criaria algo como:

Pre-v0.3 — Domain and persistence consolidation

Ela deveria ter como Definition of Done:

roadmap atualizado
domínio Release/Deployment/Runtime consolidado
SQL mutável retirado dos principais use cases
sem regressões de deploy
concorrência de deployment validada
semântica de reconciliation documentada
E2E limpo passando

A iteração atual já registra como concluídos bootstrap, ciclo stop/start/status, CI dispatcher e a regressão completa.

3. Consolidar definitivamente Release, Deployment e Runtime

Este é provavelmente o ajuste conceitual mais importante antes do reconciler.

O banco/modelo está correto, mas os tipos estão distribuídos de forma inconsistente.

Hoje existe:

domain/
└── release.rs
      Release

use_cases/
└── deployment_create.rs
      Deployment
      DeploymentType
      DeploymentStatus
      RuntimeState

Deployment, DeploymentType, DeploymentStatus e RuntimeState são conceitos centrais do domínio, mas atualmente vivem dentro de um caso de uso.

Eu transformaria em:

domain/
├── release.rs
│     └── Release
│
├── deployment.rs
│     ├── Deployment
│     ├── DeploymentType
│     └── DeploymentStatus
│
└── runtime.rs
      └── RuntimeState

Sem comportamento sofisticado, traits ou abstrações novas. Apenas ownership correto dos conceitos.

Eliminar o Release duplicado

Hoje existe:

domain::release::Release

e também:

adapters::stores::release_store::Release

com praticamente os mesmos campos.

Eu eliminaria o segundo.

O store deveria simplesmente devolver:

domain::release::Release

Isso deixa explícito que existe uma única representação conceitual de Release.

4. Corrigir os nomes que hoje borram Release e Deployment

O exemplo principal é:

pub struct DeployedRelease {
    pub deployment_id: String,
    pub runtime_id: String,
    ...
}

Esse objeto representa o resultado de um Deployment, mas se chama DeployedRelease.

Eu o renomearia para algo como:

DeploymentResult

Minha preferência seria essa, porque a função:

deploy_release(...)

passaria a significar claramente:

input:
Release

operation:
Deployment

output:
DeploymentResult

Isso reforça:

Release ≠ Deployment

sem alterar qualquer comportamento.

5. Corrigir a ambiguidade de source_revision

Existe atualmente:

let runtime_identity =
    release.source_revision.as_deref().unwrap_or(&release.id);

Ou seja, se não houver commit Git, um release.id passa a ser usado na posição de uma identidade que deriva de source_revision.

Eu não deixaria isso entrar na v0.3 assim.

A pergunta deve ser:

Para que essa string realmente existe no runtime?

Se é uma label para identificar a versão executada, ela deve ter nome como:

runtime_revision
artifact_identity
release_identity

e usar uma identidade coerente, provavelmente:

source_revision quando disponível
ou
image_digest

Mas release.id não deveria semanticamente virar uma source revision.

Isso precisa ser resolvido antes do reconciler porque reconciliation terá que identificar com precisão:

“Este runtime observado corresponde à Release que deveria estar ativa?”

6. Terminar a separação entre use cases e stores

Esse é o maior trabalho estrutural do pré-v0.3.

O próprio roadmap da v0.2 define a regra:

use cases decidem o que deve acontecer; stores SQLite decidem como persistir.

Mas isso ainda não está aplicado uniformemente.

application_import

Hoje abre uma transaction e contém diretamente SQL para:

criar/encontrar System;
criar Application;
delivery spec;
source spec;
runtime spec;
health check;
exposure.

Deveria ficar conceitualmente:

application_import
    │
    ├── valida manifest
    ├── define intenção
    │
    └── transaction
          │
          └── application_store
                ├── ensure_system
                ├── insert_application
                ├── insert_delivery_spec
                ├── insert_source_spec
                ├── insert_runtime_spec
                ├── insert_health_spec
                └── insert_exposure

A transaction continua no use case quando ele precisa garantir atomicidade do conjunto.

Isso é importante: não queremos simplesmente “esconder transactions nos stores”.

application_runtime

Este é ainda mais importante porque será reutilizado diretamente pelo reconciler.

Hoje ele possui queries próprias para carregar desired state/current runtime e updates próprios para desired state e observações do runtime.

Eu moveria essas operações para:

application_store
runtime_store

deixando application_runtime somente com:

carregar intenção
        ↓
observar Podman
        ↓
decidir
        ↓
produzir efeito
        ↓
persistir resultado

Esse é quase exatamente o padrão que o reconciler vai precisar.

exposure_change

Hoje também contém diretamente leitura e escrita das tabelas de Applications, exposures e runtimes.

Eu moveria as queries para stores antes do reconciler, porque exposição fará parte do futuro desired-vs-observed.

deployment_create

Esse já está parcialmente melhor: utiliza application_store, release_store e deployment_store, mas ainda mantém SQL próprio para verificar deployment concorrente, descobrir a Release ativa, inserir Deployment e recarregá-lo.

Eu terminaria a migração.

O resultado deveria ser:

deployment_create
      │
      ├── abre TransactionBehavior::Immediate
      ├── verifica regras
      ├── chama stores
      └── commit

e não possuir SQL.

A regra final

Eu escreveria explicitamente em architecture.md:

Use case owns:
- orchestration
- business decisions
- ordering
- transaction boundary when atomicity spans multiple writes

Store owns:
- SQL
- mapping database ↔ domain
- persistence primitives

External adapters own:
- Git
- OCI
- Podman
- systemd
- Caddy

E manteria uma regra que a arquitetura atual já possui:

Nunca manter uma transaction SQLite aberta durante Git, Podman, Caddy ou HTTP.

7. Não criar abstrações extras durante essa limpeza

Isso também precisa fazer parte do plano.

Eu não adicionaria antes da v0.3:

Repository<T>
Repository traits
UnitOfWork
Service layer
DI container
generic storage abstractions
async
event bus

Os stores concretos já existentes são suficientes.

A limpeza é:

SQL saiu do use case

e não:

vamos redesenhar toda a arquitetura de persistência

Isso mantém o estilo simples que o projeto explicitamente busca; a arquitetura atual inclusive registra ausência deliberada de traits/generics/async para essas abstrações.

8. Validar a exclusão mútua já existente

Eu não implementaria locking novo antes da v0.3.

create_deployment() já abre:

TransactionBehavior::Immediate

e dentro dela verifica se existe outro deployment não terminal da Application antes de inserir o próximo.

Isso é uma boa base para impedir:

deploy A
+
deploy A

concorrentes.

O que falta é provar isso com um teste de concorrência real, idealmente dois processos/conexões separados.

O teste deveria garantir:

processo 1
pneuma app deploy app ...

processo 2 simultâneo
pneuma app deploy app ...

resultado:

1 ganha
1 recebe ActiveDeployment

Se isso passar, marcaríamos:

deployment mutual exclusion ✅

em vez de construir outro lock.

Depois, para a v0.3, teremos que definir a interação:

deploy × reconcile

mas isso depende primeiro do design do reconciler.

9. Não mexer mais em CI/bootstrap, salvo validação/documentação

Esse bloco já pode ser considerado essencialmente concluído.

O bootstrap atual cria o ambiente operacional, usa /etc/pneuma/environment e já possui suporte à identidade CI restrita; architecture.md registra o ci_dispatch como forced-command que aceita somente deploy <app> <branch> e version.

A iteração atual também registra staging funcionando via essa identidade CI restrita.

Portanto, antes da v0.3:

não:
  redesenhar SSH
  criar pneuma-deployer
  criar API
  adicionar OIDC

sim:
  manter testes
  atualizar roadmap
  validar que production continua funcionando
10. Escrever o design do reconciliation antes do reconciler.rs

Esse é o último grande artefato obrigatório antes de começar código da v0.3.

Criaria:

docs/design/reconciliation.md

ou nome equivalente.

Esse documento precisa congelar semântica, não implementação.

Fontes de verdade

Eu definiria:

SQLite
    desired state
    active deployment
    Release identity
    histórico

Podman/systemd
    observed runtime state

Caddy
    observed exposure state

OCI registry
    artifact availability

A arquitetura atual já deixa explicitamente claro que SQLite não é fonte do estado observado do runtime; Podman é.

Matriz de runtime

Por exemplo:

Desired	Observed	Ação
Running	Running	no-op
Running	Stopped	start
Running	Missing	recover
Running	Failed	recover/report
Stopped	Running	stop
Stopped	Stopped	no-op
Stopped	Missing	no-op

Hoje já existe parte dessa semântica: a bateria atual mostra que um container removido com desired Running pode ser recriado pelo Quadlet e que status reconcilia a identidade observada; também houve correção específica para Stopped + Missing.

Matriz de exposição
desired Public
observed correct
→ no-op

desired Public
fragment missing
→ materialize

desired Public
fragment wrong
→ replace

desired Internal
fragment missing
→ no-op

desired Internal
fragment present
→ remove
Deployment recovery

Definir o que acontece se o processo morrer em:

Pending
Starting
Verifying
Activating

Minha política inicial continuaria sendo conservadora:

não sabemos se promoção concluiu?
        ↓
não promover automaticamente
        ↓
inspecionar estado real
        ↓
preservar runtime saudável anterior
        ↓
cleanup seguro
        ↓
marcar interrupted/failed
Invariantes

Documentaria pelo menos:

uma Application tem no máximo um Deployment ativo

uma Release é imutável

reconciliation não cria Release nova

runtime recovery não cria Deployment novo por padrão

reconciliation não muda desired state

reconciliation nunca escolhe versão mais nova

reconciliation não observa registry procurando release nova

reconciliation deve ser idempotente

Essa última frase define muito da v0.3:

Reconcile não decide uma nova intenção; apenas converge a realidade para uma intenção já persistida.

11. Definir a bateria de testes da v0.3 antes da implementação

Não precisa escrever todo o código de teste antecipadamente, mas os cenários têm que estar definidos.

O documento de testes deveria cobrir quatro classes:

RUNTIME DRIFT

kill container
remove container
stop unit
remove materialização Quadlet
reboot


EXPOSURE DRIFT

delete Caddy fragment
alter Caddy target
public sem route
internal com route


DEPLOYMENT RECOVERY

crash em Pending
crash em Starting
crash em Verifying
crash em Activating


CONCURRENCY / IDEMPOTENCY

reconcile duas vezes
reconcile paralelo
deploy × deploy
deploy × reconcile

Assim a v0.3 passa a ser conduzida por comportamentos esperados, e não pelo formato que reconciler.rs assumir.

12. Rodar uma regressão completa depois da consolidação

Só depois dos refactors de domínio/persistência.

Primeiro:

cargo fmt --check

cargo clippy \
  --all-targets \
  --all-features \
  -- \
  -D warnings

cargo test --all-features

cargo build --release

A iteração atual já usa esses quatro gates.

Depois, VM Debian limpa:

bootstrap
   ↓
doctor
   ↓
system create
   ↓
app import
   ↓
branch deploy
   ↓
candidate + health + promotion
   ↓
status
   ↓
stop
   ↓
start
   ↓
visibility
   ↓
rollback
   ↓
reboot
   ↓
CI dispatcher

A baseline atual é forte: bootstrap 20 PASS / 0 FAIL e bateria principal 27 PASS / 0 FAIL / 1 SKIP. O pré-v0.3 não deveria reduzir isso.

Ordem concreta que eu seguiria

Eu faria exatamente nesta sequência:

docs: redefine roadmap after v0.2 — corrigir v0.3 e dividir v0.5 em etapas futuras.
docs: open pre-v0.3 consolidation iteration — registrar escopo e DoD.
refactor(domain): make deployment and runtime first-class domain types — mover Deployment, DeploymentType, DeploymentStatus, RuntimeState.
refactor(release): use a single domain Release type — eliminar release_store::Release.
refactor(deployment): rename DeployedRelease to DeploymentResult.
refactor(runtime): separate source revision from runtime identity — resolver o fallback de release.id.
refactor(store): move application import persistence to application store.
refactor(store): move application runtime persistence to stores.
refactor(store): move exposure persistence to stores.
refactor(store): finish deployment create persistence extraction.
test(deployment): verify concurrent deployment exclusion.
docs: define reconciliation semantics and invariants.
test: define v0.3 reconciliation E2E scenarios.
regressão completa em VM limpa.
Opcionalmente publicar v0.2.1 como baseline consolidada.
Só então: primeiro commit funcional da v0.3.
O que não deve bloquear a v0.3

Eu não esperaria por:

registry watcher
idempotency-key genérica
audit trail completo do GitHub
image retention
API HTTP
TUI
OIDC
GitHub App
RBAC
novo usuário Linux de deploy

São preocupações válidas, mas nenhuma delas é necessária para implementar corretamente desired-vs-observed e recovery.

Definition of Done do pré-v0.3

Eu só começaria pneuma reconcile quando puder responder sim a estas perguntas:

Existe uma definição única e inequívoca de Release, Deployment e Runtime no código?

Os principais use cases que o reconciler reutilizará estão livres de SQL direto?

Está claro quem controla as transactions e quem controla SQL?

source_revision significa apenas source revision?

Deployment concorrente já está testado?

Sabemos exatamente qual sistema é fonte de verdade para cada estado?

Temos uma tabela dizendo o que fazer para cada combinação desired × observed?

Sabemos o que fazer com cada Deployment interrompido?

Sabemos explicitamente o que o reconciler não pode fazer?

Toda a regressão v0.2 continua verde?

Quando essas respostas forem positivas, aí a primeira implementação da v0.3 pode ser pequena e muito objetiva:

pneuma reconcile <app>
        ↓
observe
        ↓
compare
        ↓
produce ReconciliationPlan
        ↓
apply

Esse é o ponto em que eu consideraria a base realmente pronta para evoluir de “Pneuma executa mudanças” para “Pneuma mantém o estado desejado”
