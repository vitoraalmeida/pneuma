# Roadmap do Pneuma: do zero à v0.3

## 1. Objetivo da v0.3

Ao final da v0.3, o fluxo do site pessoal deverá ser:

```text
Pull request
    ↓
GitHub Actions valida o projeto
    ↓
Merge na main
    ↓
GitHub Actions constrói a imagem OCI
    ↓
Imagem é publicada no GHCR
    ↓
Pipeline resolve o digest da imagem
    ↓
GitHub Actions acessa a VPS por SSH
    ↓
Executa um comando não interativo do Pneuma
    ↓
Pneuma cria um container candidato
    ↓
Health check é executado
    ↓
Nova versão é ativada
    ↓
Versão anterior é mantida para rollback
```

O site continuará acessível em:

```text
https://vitoralmeida.tech
```

A rota pública será administrada pelo Pneuma por meio do Caddy.

---

# 2. Princípios do projeto

Desde a primeira versão, o Pneuma deve seguir estes princípios:

## 2.1 O Core não conhece interfaces

A TUI, a CLI e futuramente a API web serão apenas interfaces para os mesmos casos de uso.

```text
TUI ─┐
CLI ─┼──→ Pneuma Core ──→ Podman, Git, Caddy, SQLite, Registry
API ─┘
```

A TUI não pode executar diretamente:

```bash
podman run
git clone
caddy reload
```

Ela deve chamar casos de uso como:

```text
ImportApplication
DeployRevision
DeployRelease
StartApplication
StopApplication
ChangeExposure
RollbackDeployment
```

## 2.2 Operações devem ser idempotentes

Repetir uma operação não pode produzir recursos duplicados.

Exemplos:

- importar novamente o mesmo repositório não duplica a aplicação;
- iniciar uma aplicação já iniciada não cria outro container;
- implantar novamente o mesmo digest não cria um deployment desnecessário;
- tornar pública uma aplicação já pública não duplica a rota no Caddy.

## 2.3 Estado desejado e estado observado são diferentes

O Pneuma deve saber diferenciar:

```text
Estado desejado: Running
Estado observado: Stopped
```

Essa separação prepara o projeto para reconciliação futura.

## 2.4 A aplicação deve sobreviver ao Pneuma

Fechar a TUI ou reiniciar o serviço do Pneuma não deve interromper o site.

O runtime da aplicação deve ser supervisionado pelo host, por meio de Podman e systemd.

## 2.5 Uma release deve ser identificável

Na v0.1, a versão implantada será identificada pelo commit Git.

Na v0.2 e na v0.3, será identificada pelo digest da imagem OCI.

```text
Commit:
e48c715

Digest:
sha256:9b314...
```

## 2.6 A exposição pública é independente da execução

Uma aplicação pode estar:

```text
Running + Internal
Running + Public
Stopped + Internal
Stopped + Public configuration preserved
```

Remover a exposição pública não deve necessariamente parar a aplicação.

## 2.7 O próprio Pneuma deve ser operável e recuperável

O Pneuma também é um software de produção e precisa possuir um lifecycle explícito.

Desde a v0.1, devem estar definidos:

- instalação e atualização do binário;
- usuário, diretórios e permissões;
- backup e restauração do SQLite;
- rollback para uma versão anterior do Pneuma;
- diagnóstico básico do ambiente;
- recuperação que não dependa exclusivamente do próprio Pneuma.

O mecanismo de contingência do site deve continuar utilizável mesmo quando o Pneuma estiver indisponível.

## 2.8 Mudanças no site devem ser adotadas de forma progressiva

O site principal não deve ser usado como primeiro ambiente de validação.

O caminho esperado é:

```text
Testes automatizados
    ↓
Ambiente local ou VM descartável
    ↓
VPS com domínio temporário
    ↓
vitoralmeida.tech
```

A migração para o domínio principal deve possuir procedimento de reversão previamente testado.

## 2.9 O desenvolvimento deve produzir evidências de engenharia

A documentação e a apresentação do projeto devem evoluir com o código.

Devem ser preservados incrementalmente:

- ADRs relevantes;
- diagramas atualizados;
- demonstrações dos fluxos principais;
- exemplos de falhas e recuperação;
- releases intermediárias;
- métricas simples de build e deployment;
- limitações conhecidas e decisões revisadas.

---

# 3. Visão geral dos marcos

| Marco | Resultado principal |
|---|---|
| v0.0 | Site, servidor, ambientes e plano de migração preparados |
| v0.1 | Site importado, construído e implantado manualmente pelo Pneuma |
| v0.2 | Imagem construída no GitHub Actions e implantada manualmente por digest |
| v0.3 | Deploy automático após merge na `main`, acionado por SSH |

---

# 4. v0.0 — Preparação e fundação

## Objetivo

Preparar o site, a VPS e o repositório do Pneuma para que a primeira implementação não precise resolver problemas de infraestrutura e de domínio ao mesmo tempo.

A v0.0 termina sem uma TUI completa e sem deploy automatizado.

---

## 4.1 Containerizar o site pessoal

O repositório do site deve possuir um `Containerfile` reproduzível.

Estrutura esperada:

```text
vitoralmeida.tech/
├── Containerfile
├── pneuma.toml
├── src/
├── static/
├── templates/
├── tests/
└── README.md
```

O container deve:

- gerar ou servir o site;
- executar como usuário sem privilégios;
- expor uma porta conhecida;
- escrever logs em `stdout` e `stderr`;
- responder a uma rota usada como health check;
- não depender de arquivos presentes apenas na VPS;
- não armazenar dados importantes dentro do container.

Exemplo de contrato:

```text
Container port: 8080
Health check:   GET /
Expected:       HTTP 200
```

### Critério de saída

Este comando deve funcionar fora do Pneuma:

```bash
podman build -t personal-site:test .
podman run --rm -p 8080:8080 personal-site:test
```

E o site deve responder:

```bash
curl --fail http://127.0.0.1:8080/
```

---

## 4.2 Criar o manifesto inicial

O site deve declarar o suficiente para ser importado.

Exemplo inicial:

```toml
schema_version = 1

[application]
name = "personal-site"

[source]
repository = "https://github.com/vitoraalmeida/vitoralmeida.tech"
branch = "main"

[build]
containerfile = "Containerfile"
context = "."

[runtime]
container_port = 8080
healthcheck_path = "/"
expected_status = 200

[exposure]
default_visibility = "public"
domain = "vitoralmeida.tech"
```

O manifesto não deve conter:

- credenciais;
- tokens;
- caminhos absolutos da VPS;
- configuração interna do banco do Pneuma;
- identificadores de containers;
- endereços de rede gerados pelo runtime.

---

## 4.3 Preparar a VPS

Criar um usuário dedicado:

```text
pneuma
```

Esse usuário deverá possuir:

- diretório de trabalho próprio;
- acesso ao Podman rootless;
- permissão para administrar somente os recursos do Pneuma;
- acesso controlado à configuração de exposição;
- nenhum uso rotineiro de `root`.

Estrutura sugerida:

```text
/var/lib/pneuma/
├── database/
├── backups/
├── repositories/
├── deployments/
├── generated/
└── logs/

/etc/pneuma/
└── config.toml
```

A configuração do Caddy deverá ser dividida para permitir que o Pneuma administre apenas uma parte dela:

```text
/etc/caddy/
├── Caddyfile
└── applications/
    └── personal-site.caddy
```

O arquivo principal importa as configurações geradas:

```caddyfile
import /etc/caddy/applications/*.caddy
```

---

## 4.4 Criar o workspace Rust

Estrutura inicial:

```text
pneuma/
├── Cargo.toml
├── crates/
│   ├── pneuma-core/
│   ├── pneuma-storage-sqlite/
│   ├── pneuma-git/
│   ├── pneuma-podman/
│   ├── pneuma-caddy/
│   ├── pneuma-cli/
│   └── pneuma-tui/
├── docs/
│   ├── architecture.md
│   ├── roadmap.md
│   ├── current-iteration.md
│   └── decisions/
└── tests/
```

Responsabilidades:

| Crate | Responsabilidade |
|---|---|
| `pneuma-core` | Domínio e casos de uso |
| `pneuma-storage-sqlite` | Persistência |
| `pneuma-git` | Repositórios e revisões |
| `pneuma-podman` | Build e execução |
| `pneuma-caddy` | Exposição pública |
| `pneuma-cli` | Comandos não interativos |
| `pneuma-tui` | Interface interativa |

---

## 4.5 Definir o domínio mínimo

Entidades iniciais:

```text
Application
SourceSpecification
BuildSpecification
RuntimeSpecification
Exposure
Revision
Deployment
```

Estados mínimos:

```rust
enum DesiredRuntimeState {
    Running,
    Stopped,
}

enum ObservedRuntimeState {
    NotCreated,
    Starting,
    Running,
    Stopped,
    Failed,
    Unknown,
}

enum DeploymentStatus {
    Pending,
    Building,
    Starting,
    Verifying,
    Succeeded,
    Failed,
    RolledBack,
}
```

O domínio não deve conter tipos separados para cada string sem que exista comportamento ou invariantes associados.

Por exemplo, `Application` é um domínio. Nome, porta e repositório são propriedades ou value objects apenas quando realmente precisarem validar regras.

---

## 4.6 Criar os ports do Core

O Core deverá depender de interfaces abstratas:

```rust
trait ApplicationRepository;
trait SourceRepository;
trait ImageBuilder;
trait RuntimeManager;
trait ExposureManager;
trait HealthChecker;
trait DeploymentRepository;
```

As implementações concretas ficarão nos adapters:

```text
SourceRepository  → Git
ImageBuilder      → Podman
RuntimeManager    → Podman/systemd
ExposureManager   → Caddy
Persistence       → SQLite
```

---

## 4.7 Definir a operação do próprio Pneuma

Antes de colocar o site real sob seu controle, deve existir um contrato operacional para o Pneuma.

Definir:

- como o binário é instalado na VPS;
- se existe um processo residente na v0.1 e, quando existir, como ele é iniciado pelo systemd;
- qual usuário executa o processo;
- onde ficam configuração, banco, backups e logs;
- como uma nova versão do Pneuma é instalada;
- como voltar para o binário anterior;
- como fazer backup consistente do SQLite;
- como restaurar o banco em uma instalação limpa;
- como diagnosticar dependências e permissões.

Comandos planejados para a v0.1:

```bash
pneuma version
pneuma doctor
pneuma database backup
pneuma database restore <arquivo>
```

A recuperação de emergência do site deve possuir um procedimento externo ao Pneuma, capaz de restaurar a configuração anterior do servidor.

---

## 4.8 Definir ambientes de validação e migração

O projeto deve utilizar os seguintes níveis:

```text
Testes automatizados
    ↓
Ambiente local ou VM descartável
    ↓
VPS real com domínio temporário
    ↓
Domínio principal
```

Na VPS, reservar um domínio como:

```text
preview.vitoralmeida.tech
```

ou:

```text
pneuma-test.vitoralmeida.tech
```

Criar um plano de migração com:

1. descrição do fluxo atual de publicação;
2. execução paralela do site gerenciado pelo Pneuma;
3. validação interna;
4. validação pelo domínio temporário;
5. troca controlada do domínio principal;
6. período de observação;
7. rollback para o fluxo anterior;
8. remoção do fluxo antigo somente após estabilização.

O rollback da migração não pode depender do Pneuma estar funcional.

---

## 4.9 Definir a estratégia de evidências do portfólio

Desde a v0.0, cada marco deve produzir evidências reutilizáveis:

- release notes;
- diagramas atualizados;
- decisões arquiteturais relevantes;
- comandos de demonstração;
- registros de falhas e recuperação;
- screenshots ou gravações curtas;
- duração aproximada de build e deployment;
- limitações conhecidas.

A apresentação da v0.1 deve responder claramente:

```text
Qual problema o Pneuma resolve?
Por que não usar apenas scripts?
Quais garantias o deployment oferece?
Como o sistema reage a falhas?
Como reproduzir a demonstração?
```

---

## Critérios de saída da v0.0

A v0.0 termina quando:

- o site possui `Containerfile`;
- o site funciona localmente em um container;
- o manifesto `pneuma.toml` existe;
- a VPS possui usuário e diretórios dedicados;
- Podman rootless funciona;
- o Caddy aceita arquivos importados por aplicação;
- o workspace Rust compila;
- os ports principais estão definidos;
- pelo menos uma implementação fake permite testar o Core;
- decisões principais estão registradas em ADRs;
- instalação, atualização e rollback do Pneuma estão documentados;
- estratégia de backup e restauração do SQLite está definida;
- domínio temporário e níveis de ambiente estão definidos;
- plano de migração e reversão do site está documentado;
- estratégia de evidências do portfólio está registrada.

---

## Fora do escopo da v0.0

- TUI completa;
- deploy real;
- GitHub Actions publicando imagens;
- integração com registry;
- criação de aplicações;
- autenticação entre serviços;
- múltiplos serviços por aplicação;
- redes privadas entre projetos;
- secrets;
- API web.

---

# 5. v0.1 — Importação e deploy manual a partir do código-fonte

## Objetivo

Permitir que o Pneuma importe o site existente, construa sua imagem na VPS e gerencie seu lifecycle.

Fluxo:

```text
Repositório existente
    ↓
Importação pelo Pneuma
    ↓
Checkout de um commit específico
    ↓
Build da imagem na VPS
    ↓
Criação do container
    ↓
Health check
    ↓
Exposição pelo Caddy
```

---

## 5.1 Implementar persistência

Usar SQLite para armazenar:

```text
applications
revisions
deployments
runtime_instances
exposures
operations
```

Dados mínimos de `Application`:

```text
id
name
repository_url
default_branch
container_port
healthcheck_path
desired_state
created_at
updated_at
```

Dados mínimos de `Deployment`:

```text
id
application_id
revision
status
started_at
finished_at
failure_reason
previous_deployment_id
```

O banco deverá utilizar migrations desde o início.

---

## 5.2 Implementar importação

Caso de uso:

```text
ImportApplication
```

Entrada:

```text
URL ou caminho do repositório
```

Processamento:

1. validar o repositório;
2. clonar em diretório controlado;
3. localizar e carregar `pneuma.toml`;
4. validar nome, porta, branch e domínio;
5. impedir duplicidade;
6. persistir a aplicação;
7. registrar o commit atual;
8. não iniciar automaticamente até que a importação termine com sucesso.

Resultado:

```text
Application imported
Status: Not deployed
Revision: e48c715
```

---

## 5.3 Implementar build local

Caso de uso:

```text
BuildRevision
```

Fluxo:

1. executar `git fetch`;
2. resolver um commit específico;
3. criar checkout isolado;
4. executar o build pelo Podman;
5. usar uma tag derivada da aplicação e do commit;
6. capturar logs;
7. registrar sucesso ou falha.

Exemplo:

```text
localhost/pneuma/personal-site:e48c715
```

O Pneuma não deve construir diretamente sobre um diretório mutável compartilhado.

Cada deployment deverá utilizar um checkout ou worktree correspondente ao commit implantado.

---

## 5.4 Implementar lifecycle do container

Casos de uso:

```text
StartApplication
StopApplication
RestartApplication
GetApplicationStatus
```

Regras:

- um único runtime ativo por aplicação;
- nomes determinísticos;
- iniciar uma aplicação já iniciada não cria outro container;
- parar uma aplicação já parada não produz erro fatal;
- o estado observado vem do Podman, não apenas do SQLite;
- o container deve sobreviver ao encerramento da TUI;
- o runtime deve ser restaurado após reinício do host.

Nome conceitual:

```text
pneuma-personal-site
```

---

## 5.5 Implementar deployment

Caso de uso:

```text
DeployRevision
```

Fluxo inicial:

```text
Registrar Pending
    ↓
Resolver commit
    ↓
Build da imagem
    ↓
Registrar Building
    ↓
Criar container candidato
    ↓
Registrar Starting
    ↓
Executar health check
    ↓
Registrar Verifying
    ↓
Ativar nova versão
    ↓
Registrar Succeeded
```

Em caso de falha:

```text
Build falhou
    ou
Container não iniciou
    ou
Health check falhou
        ↓
Deployment marcado como Failed
        ↓
Versão anterior permanece ativa
```

Na primeira implementação, a troca pode exigir uma indisponibilidade curta.

Blue-green completo não é requisito da v0.1.

---

## 5.6 Implementar health check

O health check deve possuir:

- timeout por tentativa;
- número máximo de tentativas;
- intervalo entre tentativas;
- status HTTP esperado;
- registro do motivo da falha.

Exemplo:

```text
GET http://127.0.0.1:<porta>/
Expected: 200
Attempts: 10
Interval: 1 second
```

O Core deve receber apenas o resultado estruturado, sem depender diretamente de uma biblioteca HTTP específica.

---

## 5.7 Implementar integração com Caddy

Casos de uso:

```text
MakeApplicationPublic
MakeApplicationInternal
GetExposureStatus
```

Ao tornar pública:

1. validar domínio;
2. validar que a aplicação existe;
3. verificar se o runtime está disponível;
4. gerar arquivo temporário;
5. validar a configuração do Caddy;
6. substituir o arquivo definitivo atomicamente;
7. recarregar o Caddy;
8. testar o endpoint externo;
9. persistir o estado.

Exemplo gerado:

```caddyfile
vitoralmeida.tech {
    reverse_proxy 127.0.0.1:18080
}
```

Ao tornar interna:

1. remover a rota pública;
2. validar a configuração;
3. recarregar o Caddy;
4. manter a aplicação em execução;
5. persistir a nova exposição.

---

## 5.8 Implementar CLI mínima

Mesmo que a interface principal seja a TUI, a CLI deve existir desde a v0.1.

Comandos:

```bash
pneuma app import <repository>
pneuma app list
pneuma app status personal-site
pneuma app deploy personal-site --revision e48c715
pneuma app start personal-site
pneuma app stop personal-site
pneuma app expose personal-site --public
pneuma app expose personal-site --internal
pneuma deployment list personal-site
pneuma deployment rollback personal-site
```

Na v0.1, esses comandos podem ser usados para diagnóstico e testes.

Na v0.3, serão usados pelo pipeline.

---

## 5.9 Implementar TUI mínima

A primeira TUI deve priorizar operação, não estética.

Tela principal:

```text
Pneuma

Applications

> personal-site    Running    Healthy    Public

[i] Import
[d] Deploy
[s] Start
[x] Stop
[e] Exposure
[l] Logs
[r] Refresh
[q] Quit
```

Tela da aplicação:

```text
personal-site

Desired state:   Running
Observed state:  Running
Health:          Healthy
Exposure:        Public
Domain:          vitoralmeida.tech
Revision:        e48c715
Last deployment: Succeeded
```

A TUI chama os mesmos casos de uso da CLI.

---

## 5.10 Implementar rollback básico

O rollback da v0.1 deverá:

1. localizar o último deployment bem-sucedido;
2. verificar se a imagem local ainda existe;
3. iniciar a revisão anterior;
4. executar health check;
5. reativar a versão anterior;
6. registrar um novo deployment do tipo rollback.

Comando:

```bash
pneuma deployment rollback personal-site
```

---

## 5.11 Implementar operação e recuperação do Pneuma

Antes da migração do domínio principal, implementar o suporte mínimo para operar o próprio Pneuma.

### Instalação e atualização

- instalar o binário em caminho controlado;
- instalar a CLI e a TUI em caminho controlado;
- quando existir um processo residente, executá-lo por uma unidade systemd;
- quando não existir daemon, manter o systemd responsável apenas pelos runtimes e serviços do host;
- validar configuração antes de reiniciar;
- substituir o binário de forma atômica;
- preservar a versão anterior para rollback;
- registrar a versão em execução.

### Diagnóstico

Implementar:

```bash
pneuma version
pneuma doctor
```

O diagnóstico deve verificar pelo menos:

- acesso ao SQLite;
- permissões dos diretórios;
- disponibilidade do Git;
- disponibilidade e funcionamento do Podman rootless;
- acesso ao Caddy e validação de configuração;
- espaço em disco;
- capacidade de realizar health checks locais.

### Backup e restauração

Implementar:

```bash
pneuma database backup
pneuma database restore <arquivo>
```

Regras:

- backup consistente do SQLite;
- arquivo com timestamp e versão do schema;
- restauração validada em banco temporário antes da substituição;
- backup automático antes de migrations destrutivas;
- procedimento manual de restauração documentado.

### Recuperação externa

Manter um procedimento independente do Pneuma para:

- restaurar a configuração anterior do proxy;
- iniciar o site pelo fluxo anterior;
- localizar o último backup válido;
- desabilitar temporariamente um eventual serviço residente ou impedir novas operações do Pneuma.

---

## 5.12 Testes da v0.1

### Testes unitários

- validação do manifesto;
- transições de estado;
- idempotência;
- prevenção de duplicidade;
- seleção da revisão anterior;
- regras de exposição.

### Testes de integração

- SQLite real;
- repositório Git temporário;
- build de imagem de teste;
- start e stop de container;
- health check;
- geração da configuração do Caddy.

### Teste end-to-end

Cenário:

1. importar aplicação de teste;
2. realizar primeiro deployment;
3. verificar que está respondendo;
4. realizar segundo deployment;
5. provocar health check inválido;
6. confirmar que a versão anterior continua disponível;
7. reiniciar o processo do Pneuma;
8. confirmar que o estado observado é reconstruído corretamente.

### Testes operacionais

- executar backup e restauração em instalação temporária;
- atualizar o binário e retornar para a versão anterior;
- executar `pneuma doctor` com dependências saudáveis e defeituosas;
- recuperar o site usando somente o procedimento externo ao Pneuma.

---

## 5.13 Validar em ambiente temporário e ensaiar a migração

Antes de assumir `vitoralmeida.tech`, realizar um ensaio completo na VPS.

Fluxo:

```text
Site atual permanece ativo
    ↓
Pneuma executa o site em porta separada
    ↓
Validação por health check interno
    ↓
Exposição em domínio temporário
    ↓
Teste de atualização e rollback
    ↓
Ensaio de reversão para o fluxo anterior
    ↓
Troca do domínio principal
```

Critérios:

- o domínio temporário responde corretamente;
- uma revisão saudável pode ser implantada;
- uma revisão inválida não substitui a anterior;
- tornar a aplicação interna remove apenas a rota pública;
- o rollback para o fluxo anterior funciona sem o Pneuma;
- o banco pode ser restaurado a partir de backup;
- a troca do domínio principal possui passos e responsáveis claros;
- o fluxo antigo é mantido durante um período de observação.

Após a estabilização, o mecanismo anterior pode ser removido de forma planejada.

---

## 5.14 Preparar a demonstração e as evidências da v0.1

Consolidar:

- README com problema, arquitetura e execução rápida;
- diagrama atualizado da v0.1;
- ADRs que expliquem as decisões principais;
- release notes das versões alpha, beta e release candidate;
- roteiro reproduzível de demonstração;
- exemplo de deployment bem-sucedido;
- exemplo de revisão rejeitada e versão anterior preservada;
- exemplo de restart do Pneuma com reconstrução do estado;
- exemplo de backup, restauração e rollback operacional;
- vídeo curto do fluxo completo.

Registrar métricas simples:

- duração do build;
- duração do deployment;
- duração do health check;
- tempo de rollback;
- indisponibilidade observada, caso exista.

---

## Demonstração de conclusão da v0.1

```text
1. O site está rodando fora do Pneuma.
2. O site é adaptado para o contrato do Pneuma.
3. A aplicação é importada.
4. O Pneuma constrói uma revisão.
5. O site passa a ser executado pelo Pneuma.
6. A rota do Caddy é administrada pelo Pneuma.
7. Uma nova revisão é implantada manualmente.
8. Uma revisão inválida falha no health check.
9. A versão anterior permanece ativa.
10. O Pneuma é reiniciado e reconstrói o estado corretamente.
11. O backup do banco é criado e restaurado em uma instalação temporária.
12. `pneuma doctor` valida o ambiente e explica uma falha simulada.
13. O fluxo completo funciona primeiro em um domínio temporário.
14. O domínio principal é migrado com procedimento de reversão testado.
15. A demonstração e as evidências da v0.1 estão publicadas no repositório.
```

---

## Fora do escopo da v0.1

- imagens publicadas em registry;
- deploy por digest;
- deploy automático;
- GitHub Actions acessando a VPS;
- assinatura de imagem;
- SBOM;
- criação de novos projetos;
- múltiplas aplicações comunicando-se;
- autenticação entre serviços;
- blue-green sem indisponibilidade;
- reconciliação contínua.

---

# 6. v0.2 — Imagens produzidas pelo CI e deploy manual por digest

## Objetivo

Retirar o build do caminho normal de produção.

O GitHub Actions passa a produzir a imagem, e o Pneuma apenas implanta o artefato.

Fluxo:

```text
Pull request
    ↓
CI valida
    ↓
Merge na main
    ↓
CI constrói imagem
    ↓
CI publica no GHCR
    ↓
Pneuma encontra a release
    ↓
Usuário aprova pela TUI
    ↓
Pneuma implanta pelo digest
```

---

## 6.1 Criar pipeline de validação

Em pull requests:

```text
format
    ↓
lint
    ↓
testes
    ↓
geração do site
    ↓
build de teste do container
```

O pipeline deve falhar antes do merge caso o site não possa ser construído.

---

## 6.2 Criar pipeline de publicação

Após merge na `main`:

1. fazer checkout do commit;
2. executar novamente os testes necessários;
3. construir a imagem OCI;
4. adicionar tags informativas;
5. publicar no GHCR;
6. obter o digest;
7. registrar o digest no summary da execução.

Tags:

```text
ghcr.io/vitoraalmeida/vitoralmeida.tech:sha-e48c715
ghcr.io/vitoraalmeida/vitoralmeida.tech:main
```

Referência usada pelo Pneuma:

```text
ghcr.io/vitoraalmeida/vitoralmeida.tech@sha256:9b314...
```

O Pneuma nunca deve depender de `latest`.

---

## 6.3 Evoluir o manifesto

O manifesto passa a declarar a entrega por imagem:

```toml
schema_version = 1

[application]
name = "personal-site"

[source]
repository = "https://github.com/vitoraalmeida/vitoralmeida.tech"

[delivery]
type = "oci"
image = "ghcr.io/vitoraalmeida/vitoralmeida.tech"

[runtime]
container_port = 8080
healthcheck_path = "/"
expected_status = 200

[exposure]
default_visibility = "public"
domain = "vitoralmeida.tech"
```

A configuração de build local pode continuar existindo para desenvolvimento ou recuperação, mas deixa de ser o caminho padrão de produção.

---

## 6.4 Adicionar o conceito de Release

Na v0.1, a revisão Git funcionava como unidade implantável.

Na v0.2, introduzir:

```text
Release
```

Campos mínimos:

```text
id
application_id
source_revision
image_repository
image_digest
created_at
discovered_at
status
```

Relação:

```text
Commit Git
    ↓
Pipeline
    ↓
Imagem OCI
    ↓
Release
    ↓
Deployment
```

Uma release é imutável.

Um deployment é uma tentativa de colocar uma release em execução.

---

## 6.5 Implementar adapter de registry

Responsabilidades:

```text
ListReleases
ResolveDigest
PullImage
InspectImage
```

O adapter deve:

- autenticar no GHCR quando necessário;
- resolver tags em digests;
- rejeitar referência sem digest no momento do deployment;
- verificar se a arquitetura da imagem é compatível;
- registrar tamanho e metadados básicos;
- evitar downloads desnecessários.

---

## 6.6 Implementar descoberta de releases

Inicialmente, a descoberta pode ser acionada manualmente:

```bash
pneuma release refresh personal-site
```

Na TUI:

```text
personal-site

Available releases

> e48c715   sha256:9b314...   Ready
  72c9a13   sha256:63abd...   Deployed
```

O usuário seleciona:

```text
[d] Deploy selected release
```

Não é necessário implementar polling contínuo.

---

## 6.7 Implementar deploy por digest

Caso de uso:

```text
DeployRelease
```

Entrada:

```text
Application ID
Image digest
```

Fluxo:

1. validar que a release pertence à aplicação;
2. verificar se o digest já está ativo;
3. baixar a imagem;
4. executar preflight;
5. criar o container candidato;
6. executar health check;
7. ativar a release;
8. manter referência à anterior;
9. registrar o deployment.

---

## 6.8 Implementar preflight

Antes de iniciar:

```text
✓ Application exists
✓ Release exists
✓ Digest resolved
✓ Image available
✓ Runtime port available
✓ Caddy configuration valid
✓ Previous release known
✓ Required directories available
```

Falhas de configuração devem ocorrer antes da troca do runtime sempre que possível.

---

## 6.9 Descontinuar o build local como caminho principal

O build local permanece disponível apenas como modo explícito:

```bash
pneuma app deploy-source personal-site --revision e48c715
```

O deploy normal passa a ser:

```bash
pneuma app deploy personal-site \
  --image ghcr.io/vitoraalmeida/vitoralmeida.tech@sha256:9b314...
```

---

## 6.10 Testes da v0.2

Cenários adicionais:

- tag resolve para digest;
- digest inexistente;
- autenticação inválida no registry;
- arquitetura incompatível;
- imagem baixada, mas container não inicia;
- imagem inicia, mas health check falha;
- implantação repetida do mesmo digest;
- rollback para digest anterior;
- perda de conexão durante o pull.

---

## Demonstração de conclusão da v0.2

```text
1. Um commit é enviado para a main.
2. GitHub Actions constrói a imagem.
3. A imagem é publicada no GHCR.
4. O Pneuma descobre uma nova release.
5. A release aparece na TUI.
6. O deployment é aprovado manualmente.
7. A imagem é baixada por digest.
8. O health check é executado.
9. A nova release torna-se ativa.
10. O histórico relaciona commit, digest e deployment.
```

---

## Fora do escopo da v0.2

- deploy automático após merge;
- acesso do GitHub Actions à VPS;
- endpoint público de webhook;
- assinatura e verificação criptográfica;
- rollout gradual;
- canary;
- múltiplos hosts;
- fila distribuída de deployments;
- criação de aplicações por template.

---

# 7. v0.3 — Deploy automático acionado pelo GitHub Actions

## Objetivo

Fazer com que o merge na `main` termine automaticamente com a nova versão implantada pelo Pneuma.

Fluxo final:

```text
Merge na main
    ↓
Build e publicação no GHCR
    ↓
Digest obtido pelo pipeline
    ↓
SSH para a VPS
    ↓
Comando do Pneuma
    ↓
Deployment
    ↓
Health check
    ↓
Sucesso ou rollback
```

---

## 7.1 Estabilizar a CLI não interativa

Comando principal:

```bash
pneuma app deploy personal-site \
  --image ghcr.io/vitoraalmeida/vitoralmeida.tech@sha256:9b314... \
  --source-revision e48c715 \
  --non-interactive
```

A CLI deve:

- não solicitar input;
- retornar código de saída correto;
- imprimir resultado estruturado;
- produzir logs suficientes para o CI;
- aguardar a conclusão do deployment;
- falhar caso o health check falhe;
- informar se houve rollback.

Exemplo de saída:

```text
deployment_id=dep_01J...
application=personal-site
release=sha256:9b314...
status=succeeded
previous_release=sha256:63abd...
```

---

## 7.2 Implementar exclusão mútua de deployment

Uma aplicação não pode receber dois deployments simultaneamente.

Possíveis estados:

```text
No deployment running
Deployment in progress
Deployment completed
Deployment failed
```

Ao receber uma segunda solicitação:

```text
Deployment already in progress for personal-site
```

O lock deve sobreviver a falhas do processo por meio de estado persistente e recuperação.

---

## 7.3 Implementar chave de idempotência

O pipeline pode repetir uma etapa devido a timeout ou retry.

A chamada deve aceitar:

```bash
--idempotency-key github-run-123456789
```

Se a mesma chave for enviada novamente, o Pneuma retorna o resultado do deployment anterior em vez de criar outro.

---

## 7.4 Criar usuário de deploy dedicado

Criar um usuário como:

```text
pneuma-deployer
```

Ele não deve:

- possuir shell administrativo geral;
- possuir acesso irrestrito ao Podman;
- editar arquivos do Caddy diretamente;
- executar comandos arbitrários como root;
- acessar dados de outras aplicações.

O acesso deve permitir somente o comando do Pneuma.

Opções:

```text
SSH forced command
```

ou:

```text
sudoers restrito a um wrapper específico
```

Fluxo:

```text
GitHub Actions
    ↓ SSH
pneuma-deployer
    ↓
pneuma app deploy ...
    ↓
Pneuma Core
```

---

## 7.5 Configurar autenticação SSH

No GitHub:

- chave privada armazenada como secret;
- host da VPS;
- usuário de deploy;
- host key conhecida e fixada;
- nenhuma desativação de verificação de host.

Na VPS:

- chave pública dedicada ao repositório ou ambiente;
- restrições no `authorized_keys`;
- logs de autenticação;
- possibilidade de revogação independente.

A chave não deve ser reutilizada para administração pessoal da VPS.

---

## 7.6 Adicionar etapa de deploy ao workflow

Pipeline conceitual:

```yaml
jobs:
  validate:
    ...

  publish:
    needs: validate
    ...
    outputs:
      image_digest: ...

  deploy:
    needs: publish
    environment: production
    ...
```

A etapa de deploy recebe:

```text
Application: personal-site
Source revision: commit SHA
Image digest: digest produzido pelo build
Idempotency key: GitHub run ID
```

O pipeline não deve enviar uma tag mutável como artefato final.

---

## 7.7 Implementar deployment candidato

Para minimizar risco, a nova versão deve ser iniciada antes da versão anterior ser removida.

Modelo inicial:

```text
personal-site-current
personal-site-candidate
```

Fluxo:

1. versão atual continua atendendo;
2. container candidato é iniciado em uma porta temporária;
3. health check interno é executado;
4. Caddy passa a apontar para o candidato;
5. endpoint público é verificado;
6. candidato torna-se atual;
7. versão anterior é parada;
8. versão anterior permanece disponível para rollback por um período ou quantidade limitada.

Esse é um blue-green simplificado.

---

## 7.8 Implementar atualização atômica do Caddy

A troca de upstream deve seguir:

1. escrever nova configuração em arquivo temporário;
2. executar validação;
3. substituir o arquivo definitivo;
4. recarregar o Caddy;
5. testar a URL pública;
6. confirmar deployment.

Em caso de falha externa:

1. restaurar configuração anterior;
2. recarregar o Caddy;
3. verificar versão anterior;
4. marcar deployment como `RolledBack`.

---

## 7.9 Implementar rollback automático

O rollback automático ocorre quando:

- container candidato não inicia;
- health check interno falha;
- configuração do Caddy é inválida;
- reload do Caddy falha;
- health check público falha após a troca.

Estados:

```text
Pending
Pulling
Starting
VerifyingInternal
SwitchingTraffic
VerifyingExternal
Succeeded
RollingBack
RolledBack
Failed
```

O pipeline deve receber falha mesmo quando o rollback é bem-sucedido, pois a nova versão não foi entregue.

Exemplo:

```text
status=rolled_back
failed_release=sha256:9b314...
active_release=sha256:63abd...
reason=external health check failed
```

---

## 7.10 Implementar auditoria

Cada deployment deve registrar:

```text
application
source revision
image digest
GitHub workflow
GitHub run ID
requested by
request timestamp
deployment start
deployment finish
previous release
result
failure reason
rollback result
```

A TUI deve apresentar:

```text
Deployment history

e48c715  sha256:9b314...  Succeeded
72c9a13  sha256:63abd...  Active previously
431ef81  sha256:a8d11...  RolledBack
```

---

## 7.11 Implementar retenção

Definir política simples:

```text
Manter:
- release ativa;
- release anterior;
- três deployments históricos recentes;
- logs de falhas.
```

Remover imagens antigas apenas quando:

- não estiverem ativas;
- não forem necessárias para rollback;
- não estiverem associadas a deployment em andamento.

---

## 7.12 Testes da v0.3

### Pipeline

- publicação da imagem;
- captura correta do digest;
- erro antes do deploy quando build falha;
- erro quando SSH falha;
- erro quando Pneuma rejeita o deployment;
- sucesso propagado ao GitHub Actions.

### Segurança

- usuário de deploy não executa comando arbitrário;
- tag mutável é rejeitada;
- aplicação desconhecida é rejeitada;
- digest de outro repositório é rejeitado;
- chave SSH revogada deixa de funcionar;
- host key incorreta interrompe o pipeline.

### Resiliência

- duas execuções simultâneas;
- retry com mesma chave de idempotência;
- processo do Pneuma encerrado durante deployment;
- falha durante pull;
- falha durante health check;
- falha durante troca do Caddy;
- falha após troca de tráfego;
- rollback para imagem inexistente.

---

## Demonstração de conclusão da v0.3

A demonstração final deve mostrar:

```text
1. O site está na release A.
2. Um commit válido é enviado para a main.
3. GitHub Actions executa testes.
4. A imagem B é publicada no GHCR.
5. O digest de B é obtido.
6. O pipeline acessa a VPS por SSH.
7. O Pneuma recebe a solicitação.
8. O container candidato B é iniciado.
9. O health check interno passa.
10. O Caddy passa a apontar para B.
11. O health check externo passa.
12. B torna-se a release ativa.
13. A TUI exibe o novo deployment.
```

Também deve existir uma demonstração de falha:

```text
1. O site está na release B.
2. Uma imagem C com health check inválido é publicada.
3. O pipeline solicita o deployment.
4. O Pneuma inicia C.
5. O health check falha.
6. C não recebe tráfego.
7. B continua ativa.
8. O deployment é marcado como RolledBack ou Failed.
9. O GitHub Actions termina com erro.
```

---

# 8. Arquitetura esperada ao final da v0.3

```text
GitHub Repository
    │
    ├── Pull Requests
    │       └── Validation Workflow
    │
    └── Main Branch
            └── Build and Publish
                    │
                    ▼
                  GHCR
                    │
                    │ digest
                    ▼
              GitHub Actions
                    │
                    │ SSH
                    ▼
            pneuma-deployer
                    │
                    ▼
               Pneuma CLI
                    │
                    ▼
               Pneuma Core
              ┌─────┼─────┬────────┐
              ▼     ▼     ▼        ▼
           SQLite Podman Caddy  Health Check
                    │
                    ▼
               personal-site
```

---

# 9. Modelo de dados ao final da v0.3

## Application

Representa a aplicação gerenciada.

```text
id
name
repository
desired_state
runtime_configuration
health_configuration
created_at
updated_at
```

## Release

Representa um artefato imutável.

```text
id
application_id
source_revision
image_repository
image_digest
created_at
```

## Deployment

Representa uma tentativa de ativar uma release.

```text
id
application_id
release_id
previous_release_id
status
requested_by
idempotency_key
started_at
finished_at
failure_reason
```

## RuntimeInstance

Representa uma instância observada.

```text
id
application_id
release_id
runtime_identifier
role
observed_state
created_at
```

O papel pode ser:

```text
Current
Candidate
Previous
```

## Exposure

Representa a decisão de exposição.

```text
application_id
visibility
domain
active_upstream
updated_at
```

---

# 10. Documentação obrigatória

## `docs/architecture.md`

Deve conter:

- visão geral;
- limites dos componentes;
- domínio;
- ports e adapters;
- fluxo de importação;
- fluxo de deployment;
- fluxo de rollback;
- fontes de verdade.

## `docs/roadmap.md`

Deve conter este roadmap e o estado de cada marco.

## `docs/current-iteration.md`

Deve conter somente:

- objetivo atual;
- escopo;
- tarefas;
- decisões pendentes;
- critérios de conclusão;
- riscos conhecidos.

## `docs/operations/`

Deve conter:

```text
installation.md
update-and-rollback.md
backup-and-restore.md
doctor.md
emergency-recovery.md
```

## `docs/environments.md`

Deve descrever:

- testes automatizados;
- ambiente local ou VM descartável;
- domínio temporário na VPS;
- domínio principal;
- critérios de promoção entre ambientes.

## `docs/migration-plan.md`

Deve conter:

- estado atual;
- estado futuro;
- passos da migração;
- validações antes da troca;
- período de observação;
- gatilhos de rollback;
- procedimento de reversão independente do Pneuma.

## `docs/demo-v0.1.md`

Deve conter o roteiro reproduzível da demonstração e apontar para as evidências geradas durante o desenvolvimento.

## ADRs iniciais

```text
0001-use-rust.md
0002-use-podman.md
0003-use-sqlite.md
0004-separate-core-from-interfaces.md
0005-use-caddy-for-public-exposure.md
0006-use-oci-digests-for-deployments.md
0007-use-ssh-for-v0.3-delivery.md
0008-distinguish-desired-and-observed-state.md
```

## `docs/threat-model.md`

Até a v0.3, cobrir:

- imagem maliciosa;
- digest adulterado;
- usuário SSH comprometido;
- execução arbitrária no host;
- path traversal durante importação;
- injeção de comandos;
- alteração indevida do Caddy;
- roubo de credencial do registry;
- replay de deployment;
- exposição pública acidental;
- container privilegiado;
- acesso indevido ao socket do Podman.

---

# 11. Backlog sugerido em ordem

## Marco v0.0

1. Adicionar `Containerfile` ao site.
2. Criar health check reproduzível.
3. Criar `pneuma.toml`.
4. Validar build e execução com Podman.
5. Preparar usuário rootless na VPS.
6. Separar configuração do Caddy por aplicação.
7. Criar workspace Rust.
8. Definir domínio mínimo.
9. Definir ports do Core.
10. Criar SQLite com migrations.
11. Definir instalação, atualização e rollback do Pneuma.
12. Definir backup e restauração do SQLite.
13. Reservar domínio temporário e documentar os ambientes.
14. Criar plano de migração e reversão do site.
15. Definir estratégia de evidências do portfólio.

## Marco v0.1

16. Implementar parser do manifesto.
17. Implementar importação de repositório.
18. Implementar resolução de commit.
19. Implementar build local.
20. Implementar criação do runtime.
21. Implementar start, stop e status.
22. Implementar health check.
23. Implementar deployment por revisão.
24. Implementar integração com Caddy.
25. Implementar rollback básico.
26. Implementar CLI.
27. Implementar TUI.
28. Implementar `pneuma version` e `pneuma doctor`.
29. Implementar backup e restauração do banco.
30. Implementar instalação, atualização e rollback do binário.
31. Documentar recuperação externa ao Pneuma.
32. Criar teste end-to-end.
33. Validar o fluxo em domínio temporário.
34. Ensaiar migração e reversão do site.
35. Migrar o domínio principal para o Pneuma.
36. Consolidar documentação, métricas e vídeo da v0.1.

## Marco v0.2

37. Criar workflow de validação.
38. Criar workflow de build e publicação.
39. Publicar imagem no GHCR.
40. Introduzir entidade `Release`.
41. Implementar adapter de registry.
42. Implementar descoberta de releases.
43. Implementar deployment por digest.
44. Adicionar preflight.
45. Adaptar TUI para seleção de releases.
46. Tornar build local um caminho secundário.
47. Criar testes end-to-end com registry.

## Marco v0.3

48. Estabilizar CLI não interativa.
49. Implementar lock de deployment.
50. Implementar chave de idempotência.
51. Criar usuário `pneuma-deployer`.
52. Restringir acesso SSH.
53. Adicionar etapa de deploy ao GitHub Actions.
54. Implementar container candidato.
55. Implementar troca atômica do Caddy.
56. Implementar health check externo.
57. Implementar rollback automático.
58. Implementar auditoria completa.
59. Implementar retenção de releases.
60. Criar testes de falha e recuperação.
61. Atualizar demonstração e evidências do portfólio.
62. Marcar a release `v0.3.0`.

---

# 12. O primeiro passo imediato

O primeiro trabalho não deve acontecer no repositório do Pneuma.

Deve acontecer no site pessoal:

```text
1. Adicionar ou revisar o Containerfile.
2. Definir a porta interna.
3. Garantir execução sem root.
4. Garantir que GET / retorne 200.
5. Criar o pneuma.toml.
6. Validar build e execução com Podman.
7. Registrar como o site é publicado e restaurado atualmente.
8. Reservar um domínio temporário para a validação na VPS.
9. Definir o diretório, usuário e procedimento de backup do Pneuma.
```

Somente depois disso deve ser criado o primeiro caso de uso do Pneuma:

```text
ImportApplication
```

O primeiro marco técnico demonstrável será:

> O Pneuma recebe o repositório do site, lê seu manifesto, persiste a aplicação e informa que ela está pronta para seu primeiro deployment.

O primeiro marco operacional será:

> O site `vitoralmeida.tech` está sendo executado e exposto pelo Pneuma a partir de um commit específico.

O primeiro marco de entrega contínua será:

> Um merge na `main` produz uma imagem imutável e faz com que o Pneuma implante automaticamente essa release com health check e rollback.
