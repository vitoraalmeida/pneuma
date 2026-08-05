# Plano Ágil para Entrega da v0.1 do Pneuma

O roadmap deve funcionar como **direção de produto**, não como uma lista de componentes que serão implementados sequencialmente. Para chegar à v0.1 com práticas ágeis, transforme-o em incrementos verticais que produzam um comportamento utilizável a cada etapa.

A regra principal será:

> Cada iteração termina com algo executável, testado e demonstrável, mesmo que ainda incompleto.

# 1. Fixe o contrato da v0.1

Antes de abrir as primeiras issues, registre exatamente o que significa “v0.1 concluída”.

## Objetivo

> Importar o repositório containerizado do site pessoal, implantar um commit específico na VPS, controlar seu lifecycle, verificar sua saúde e administrar sua exposição pública pelo Caddy.

## Jornada obrigatória

```text
Abrir o Pneuma
    ↓
Importar o repositório do site
    ↓
Validar o pneuma.toml
    ↓
Registrar a aplicação
    ↓
Selecionar um commit
    ↓
Construir a imagem na VPS
    ↓
Iniciar o container
    ↓
Executar health check
    ↓
Expor pelo Caddy
    ↓
Consultar status
    ↓
Parar e iniciar novamente
```

## Critério final de aceite

A v0.1 só está pronta quando for possível demonstrar, em uma instalação limpa:

1. importar o site;
2. implantar um commit específico;
3. acessar `vitoralmeida.tech`;
4. consultar estado real do container;
5. parar e iniciar o site;
6. implantar uma nova revisão;
7. rejeitar uma revisão com health check inválido;
8. preservar ou restaurar a versão anterior;
9. reiniciar o Pneuma sem perder o catálogo;
10. reconstruir o estado observado a partir do Podman.

## Não objetivos da v0.1

Deixe explicitamente fora:

- GHCR;
- deploy por digest;
- GitHub Actions acessando a VPS;
- criação de aplicações por template;
- API web;
- múltiplos hosts;
- múltiplos serviços por aplicação;
- autenticação entre aplicações;
- assinatura de imagens;
- canary;
- Kubernetes.

Isso evita que a v0.1 seja continuamente expandida.

---

# 2. Transforme o roadmap em épicos

Organize as tarefas da v0.0 e da v0.1 em épicos orientados a capacidades.

| Épico | Partes do roadmap |
|---|---|
| E0 — Site executável em container | v0.0: Containerfile, porta e health check |
| E1 — Fundação do Pneuma | workspace, domínio, ports e SQLite |
| E2 — Catálogo e importação | manifesto, clone e persistência |
| E3 — Build por revisão | Git, checkout isolado e Podman build |
| E4 — Runtime | start, stop, inspect e estado observado |
| E5 — Deployment seguro | health check, histórico e rollback |
| E6 — Exposição | arquivos do Caddy, validação e reload |
| E7 — Interfaces | CLI e TUI |
| E8 — Hardening da v0.1 | idempotência, recuperação e testes E2E |

Esses épicos não devem ser concluídos isoladamente. Eles servem apenas para organizar o backlog.

Por exemplo, não implemente todo o adapter do Podman antes de existir um caso de uso que o utilize.

---

# 3. Crie um cenário de aceitação automatizável

Antes da implementação, crie um documento ou script que represente o teste final.

Por exemplo:

```text
Given:
- uma VPS com Podman e Caddy;
- o Pneuma instalado;
- nenhum site registrado;
- um repositório compatível com pneuma.toml.

When:
- o repositório é importado;
- o commit A é implantado;
- a exposição pública é ativada.

Then:
- o site responde HTTP 200;
- o deployment A está marcado como ativo;
- o estado observado é Running;
- a rota do Caddy aponta para o runtime correto.
```

Depois:

```text
When:
- o commit B, com health check inválido, é implantado.

Then:
- o deployment B falha;
- o commit A continua ativo;
- o site permanece acessível;
- a causa da falha é registrada.
```

Esse teste será inicialmente manual. Ao longo das iterações, transforme suas etapas em um script como:

```text
scripts/acceptance-v0.1.sh
```

O script não precisa nascer completo. Ele evolui com a implementação.

---

# 4. Defina seu processo de trabalho

Como o projeto será inicialmente desenvolvido por uma pessoa, não é necessário reproduzir Scrum corporativo.

Use um fluxo simples:

```text
Backlog
    ↓
Ready
    ↓
In Progress
    ↓
Review
    ↓
Done
```

## Limite de trabalho em andamento

Mantenha no máximo:

- uma issue principal em desenvolvimento;
- uma correção pequena paralela, quando indispensável.

Não comece a TUI enquanto o deployment pela CLI ainda não funcionar.

## Issues pequenas

Cada issue deve gerar:

- um comportamento observável;
- um PR;
- testes;
- documentação mínima;
- uma demonstração.

Evite issues como:

```text
Implementar integração com Podman
```

Prefira:

```text
Permitir consultar o estado real do container de uma aplicação
```

Ou:

```text
Impedir que StartApplication crie um segundo container
```

---

# 5. Adote um formato padrão para as issues

Cada issue deve conter:

```markdown
## Problema

Qual comportamento ou limitação esta issue resolve?

## Resultado para o usuário

O que será possível fazer depois desta alteração?

## Escopo

Quais mudanças fazem parte desta issue?

## Critérios de aceite

- [ ] ...
- [ ] ...
- [ ] ...

## Testes

Como o comportamento será validado?

## Fora do escopo

O que deliberadamente não será implementado?

## Dependências

Quais decisões ou issues precisam estar concluídas?
```

Exemplo para a primeira capacidade do Pneuma:

```markdown
## Problema

O Pneuma ainda não consegue registrar uma aplicação existente.

## Resultado para o usuário

O usuário pode importar um repositório local contendo um pneuma.toml.

## Critérios de aceite

- [ ] O manifesto é lido e validado.
- [ ] A aplicação é persistida no SQLite.
- [ ] Importar novamente não duplica o registro.
- [ ] Um manifesto inválido produz erro compreensível.
- [ ] `pneuma app list` mostra a aplicação importada.

## Fora do escopo

- Clone remoto.
- Build da imagem.
- Execução do container.
- TUI.
```

---

# 6. Defina Definition of Ready e Definition of Done

## Definition of Ready

Uma issue só entra em `Ready` quando:

- descreve um resultado observável;
- possui critérios de aceite;
- possui escopo e não escopo;
- não depende de uma decisão arquitetural desconhecida;
- pode ser entregue em um único PR razoavelmente pequeno;
- tem uma abordagem de teste identificada.

## Definition of Done

Uma issue só está concluída quando:

- critérios de aceite foram atendidos;
- testes relevantes foram adicionados;
- CI está verde;
- erros possuem contexto suficiente;
- código passou por revisão, mesmo que seja uma autorrevisão;
- documentação foi atualizada quando necessário;
- nenhuma etapa manual oculta é necessária;
- o comportamento pode ser demonstrado;
- não existem warnings novos;
- a branch foi integrada à `main`.

Para uma funcionalidade de runtime, acrescente:

- comportamento idempotente validado;
- falha e recuperação testadas;
- estado observado consultado no sistema real;
- recursos temporários removidos.

---

# 7. Configure a base de engenharia antes das features

O primeiro PR do repositório do Pneuma deve configurar o mínimo necessário.

## Branch e integração

Use:

- `main` sempre estável;
- branches curtas;
- pull requests pequenos;
- integração frequente;
- squash merge, se quiser um histórico mais simples.

Não crie branches de longa duração como:

```text
develop
release/v0.1
feature/all-podman-support
```

A v0.1 deve evoluir diretamente na `main`, protegida pelo CI.

## CI mínimo

Para cada PR:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --all-targets
```

Para o site:

```text
testes do sitegen
    ↓
geração do site
    ↓
build do Containerfile
    ↓
execução do container
    ↓
GET / retorna 200
```

Não adicione todos os scanners e verificadores imediatamente. Primeiro garanta que o pipeline seja rápido, confiável e utilizado em todos os PRs.

## Migrations

SQLite deve usar migrations desde o primeiro schema. Não use código que cria tabelas informalmente durante a inicialização.

## Logs estruturados

Comece usando `tracing`.

Cada operação importante deve registrar:

- application ID;
- deployment ID;
- revisão;
- operação;
- resultado;
- causa da falha.

Não é necessário configurar uma plataforma de observabilidade na v0.1.

---

# 8. Tome apenas as decisões arquiteturais necessárias

Antes do primeiro caso de uso, registre as decisões que seriam caras de mudar:

1. Rust;
2. SQLite;
3. Podman rootless;
4. Caddy para exposição;
5. Core independente de CLI e TUI;
6. commit Git como unidade implantável na v0.1;
7. estado desejado separado do observado;
8. aplicação supervisionada independentemente da TUI.

Não tente definir antecipadamente:

- todas as entidades futuras;
- formato de múltiplos serviços;
- modelo de secrets;
- API web;
- comunicação entre hosts;
- criação por templates;
- registry;
- assinatura de imagens.

Crie um ADR quando existir uma decisão real com alternativas relevantes. Não transforme cada detalhe em ADR.

---

# 9. Faça spikes para os maiores riscos

Antes de construir o fluxo completo, valide as partes com maior incerteza.

Cada spike deve ser curto, descartável e produzir uma conclusão documentada.

## Spike 1 — Podman rootless e persistência

Provar que:

- o usuário `pneuma` consegue iniciar um container;
- o container continua executando após o comando terminar;
- o runtime sobrevive ao logout SSH;
- o runtime pode ser restaurado no boot;
- `podman inspect` retorna o estado esperado.

Resultado esperado:

```text
docs/spikes/podman-rootless.md
```

## Spike 2 — Troca segura no Caddy

Provar que:

- uma configuração por aplicação pode ser criada;
- `caddy validate` detecta configuração inválida;
- reload não interrompe a configuração anterior;
- uma rota pode ser removida sem parar o container;
- o usuário do Pneuma possui apenas a permissão necessária.

## Spike 3 — Git por revisão

Provar que:

- o Pneuma consegue resolver um commit;
- criar um checkout ou worktree isolado;
- construir duas revisões diferentes;
- evitar que um deployment altere o diretório de outro.

## Spike 4 — Rollback mínimo

Com dois containers simples:

- iniciar A;
- iniciar B;
- verificar B;
- falhar o health check de B;
- preservar A.

Esses spikes reduzem o risco do roadmap sem gerar uma arquitetura prematura.

---

# 10. Execute a v0.1 em incrementos verticais

## Iteração 0 — Preparar o caso real

### Ligação com o roadmap

- v0.0: containerizar o site;
- criar manifesto;
- preparar VPS;
- validar Podman e Caddy.

### Entrega

O site ainda não é operado pelo Pneuma, mas pode ser executado manualmente com seu contrato definitivo:

```bash
podman build -t personal-site:test .
podman run --rm -p 8080:8080 personal-site:test
curl --fail http://127.0.0.1:8080/
```

### Critério de conclusão

- Containerfile reproduzível;
- container sem root;
- health check funcional;
- `pneuma.toml` válido;
- Caddy consegue encaminhar tráfego ao container;
- procedimento documentado.

Esse é o baseline de comparação. Quando o Pneuma assumir o site, o comportamento externo não deverá mudar.

---

## Iteração 1 — Walking skeleton de importação

Um walking skeleton atravessa todas as camadas necessárias, mas implementa o mínimo de comportamento.

### Fluxo

```text
CLI
    ↓
ImportApplication
    ↓
Parser do manifesto
    ↓
SQLite
    ↓
ListApplication
```

### Entrega

```bash
pneuma app import /caminho/do/site
pneuma app list
pneuma app status personal-site
```

A aplicação aparece como:

```text
Registered
Not deployed
Observed state: NotCreated
```

### Implementar

- workspace mínimo;
- domínio `Application`;
- parser do manifesto;
- migration inicial;
- repositório SQLite;
- CLI mínima;
- erros estruturados;
- idempotência de importação.

### Não implementar

- clone remoto;
- Podman;
- build;
- Caddy;
- TUI.

### Critério de conclusão

Em um banco vazio, importar duas vezes resulta em uma única aplicação.

---

## Iteração 2 — Deploy de um commit pela CLI

### Fluxo

```text
pneuma app deploy
    ↓
Resolver revisão
    ↓
Criar checkout isolado
    ↓
Podman build
    ↓
Podman run
    ↓
Registrar deployment
```

### Entrega

```bash
pneuma app deploy personal-site --revision e48c715
```

Depois:

```bash
pneuma app status personal-site
```

Retorna:

```text
Desired state: Running
Observed state: Running
Revision: e48c715
```

### Implementar

- adapter Git;
- resolução de commit;
- checkout isolado;
- build da imagem;
- criação do container;
- consulta real ao Podman;
- persistência do deployment;
- start, stop e status.

### Não implementar

- Caddy;
- rollback automático;
- TUI;
- atualização automática.

### Critério de conclusão

O site responde diretamente em uma porta local administrada pelo Pneuma.

---

## Iteração 3 — Health check e falha segura

### Fluxo

```text
Criar candidato
    ↓
Iniciar
    ↓
Executar health check
    ↓
Ativar ou rejeitar
```

### Entrega

O Pneuma diferencia:

```text
Container iniciou
```

de:

```text
Deployment foi bem-sucedido
```

### Implementar

- estados do deployment;
- timeout;
- tentativas;
- motivo estruturado de falha;
- candidato e versão anterior;
- rollback básico;
- histórico de deployments.

### Cenário obrigatório

1. implantar revisão A saudável;
2. implantar revisão B inválida;
3. B falha;
4. A continua atendendo;
5. falha de B aparece no histórico.

### Critério de conclusão

Uma revisão defeituosa não deixa o site indisponível.

---

## Iteração 4 — Exposição pelo Caddy

### Fluxo

```text
Aplicação saudável
    ↓
Gerar configuração temporária
    ↓
Validar
    ↓
Aplicar
    ↓
Verificar URL pública
```

### Entrega

```bash
pneuma app expose personal-site --public
pneuma app expose personal-site --internal
```

### Implementar

- `Exposure`;
- geração determinística do Caddyfile;
- atualização atômica;
- validação;
- reload;
- verificação externa;
- persistência do estado.

### Cenário obrigatório

```text
Running + Public
    ↓
Change exposure
    ↓
Running + Internal
```

O container continua executando.

### Critério de conclusão

O site passa a ser efetivamente publicado pelo Pneuma em `vitoralmeida.tech`.

---

## Iteração 5 — TUI como cliente fino

Somente agora implemente a TUI.

### Motivo

A CLI já terá validado:

- casos de uso;
- erros;
- contratos;
- integração;
- automação;
- comportamento não interativo.

A TUI apenas apresenta essas capacidades.

### Entrega

```text
Pneuma

> personal-site    Running    Healthy    Public

[d] Deploy
[s] Start
[x] Stop
[e] Exposure
[h] History
[r] Refresh
[q] Quit
```

### Regra

Nenhum comando Podman, Git ou Caddy deve estar dentro da camada de apresentação.

### Critério de conclusão

Todas as operações principais podem ser executadas pela TUI, mas continuam disponíveis na CLI.

---

## Iteração 6 — Hardening e release candidate

Esta iteração não adiciona uma grande feature. Ela prepara a v0.1 para ser confiável.

### Validar

- importar duas vezes;
- iniciar duas vezes;
- parar duas vezes;
- implantar a revisão já ativa;
- reiniciar o Pneuma;
- reiniciar a VPS;
- encontrar container ausente;
- encontrar container inesperado;
- banco indisponível;
- build interrompido;
- health check com timeout;
- configuração inválida do Caddy;
- rollback com falha.

### Entregas

- script de aceitação;
- documentação de instalação;
- documentação operacional;
- mensagens de erro revisadas;
- migrations testadas;
- limpeza de imagens e checkouts temporários;
- threat model inicial;
- release notes.

### Critério de conclusão

O cenário de aceitação da v0.1 funciona a partir de uma instalação limpa.

---

# 11. Use checkpoints internos de release

Não espere terminar toda a v0.1 para versionar.

Sugestão:

```text
v0.1.0-alpha.1
Importação e catálogo

v0.1.0-alpha.2
Build, start, stop e status

v0.1.0-alpha.3
Health check e rollback

v0.1.0-alpha.4
Exposição pelo Caddy

v0.1.0-beta.1
TUI completa

v0.1.0-rc.1
Hardening e teste de aceitação

v0.1.0
Site operado pelo Pneuma
```

Cada versão deve possuir:

- tag;
- release notes;
- instruções de demonstração;
- limitações conhecidas.

Isso torna a evolução do projeto visível no portfólio.

---

# 12. Faça uma revisão curta após cada incremento

Ao terminar uma iteração, responda por escrito:

```text
O que foi entregue?
O que foi mais difícil?
Qual hipótese estava errada?
Que dívida técnica foi criada?
Existe risco novo?
O próximo incremento ainda é o mais importante?
```

Registre apenas decisões úteis no `current-iteration.md`.

Não transforme a retrospectiva em um relatório extenso.

---

# 13. Estrutura documental recomendada

```text
docs/
├── product/
│   ├── vision.md
│   ├── v0.1-scope.md
│   └── acceptance-v0.1.md
├── architecture/
│   ├── architecture.md
│   └── threat-model.md
├── decisions/
│   └── 0001-*.md
├── spikes/
│   ├── podman-rootless.md
│   ├── caddy-reload.md
│   └── git-worktree.md
├── iterations/
│   └── current-iteration.md
└── roadmap.md
```

Cada arquivo possui uma função:

- `roadmap.md`: direção de médio prazo;
- `v0.1-scope.md`: compromisso da release;
- `acceptance-v0.1.md`: prova de que a release terminou;
- `architecture.md`: estrutura atual, não arquitetura futura imaginada;
- ADRs: decisões difíceis de reverter;
- `current-iteration.md`: somente o trabalho em andamento.

---

# 14. Ordem concreta antes de começar

Execute nesta sequência:

1. criar `docs/product/v0.1-scope.md`;
2. criar `docs/product/acceptance-v0.1.md`;
3. definir Definition of Ready e Definition of Done;
4. criar milestone `v0.1`;
5. criar os oito épicos;
6. configurar o board com limite de WIP;
7. proteger a branch `main`;
8. criar CI mínimo do Pneuma;
9. concluir os spikes de Podman, Caddy e Git;
10. containerizar e validar o site fora do Pneuma;
11. criar somente as issues da primeira iteração;
12. implementar o walking skeleton de importação;
13. demonstrar e revisar;
14. detalhar a iteração seguinte somente depois da anterior.

O primeiro código funcional do Pneuma deve produzir este resultado:

```bash
pneuma app import ./personal-site
pneuma app list
```

```text
personal-site    Registered    Not deployed
```

Esse incremento parece pequeno, mas valida o manifesto, o domínio, a persistência, a CLI, a arquitetura e a estratégia de testes. A partir dele, cada nova iteração adiciona uma capacidade ao mesmo fluxo, até o site estar integralmente operado pelo Pneuma na v0.1.
