# Contexto do Sistema — Pneuma v0.1

> **Arquivado:** hipótese original da Iteração D0, preservada como registro. A arquitetura implementada está em [`docs/architecture/architecture.md`](../architecture/architecture.md).

**Status:** Hipótese de design da Iteração D0  
**Nível:** contexto e fronteiras externas  
**Documentos relacionados:** [`architecture-d0.md`](./architecture-d0.md), [`domain-model.md`](./domain-model.md)

## 1. Visão

O Pneuma é um control plane local para registrar, implantar e operar aplicações containerizadas em um único servidor Linux.

Na v0.1, ele é utilizado pelo próprio administrador da VPS por SSH. O sistema coordena Git, Podman, systemd, SQLite, Caddy e health checks, mas não substitui essas tecnologias.

## 2. Fronteira do sistema

Dentro da fronteira do Pneuma estão:

- casos de uso;
- regras de domínio;
- coordenação de deployments;
- persistência do estado desejado e histórico;
- interfaces CLI e TUI;
- adapters para sistemas externos;
- geração de configuração controlada.

Fora da fronteira estão:

- repositório Git;
- engine e storage de imagens do Podman;
- systemd;
- Caddy;
- DNS;
- aplicação executada;
- sistema de arquivos do host;
- operador humano;
- fluxo anterior de contingência.

## 3. Diagrama de contexto

```mermaid
flowchart LR
    Operator["Operador / Desenvolvedor"]
    Repo["Repositório Git da aplicação"]
    Pneuma["Pneuma v0.1"]
    SQLite[("SQLite")]
    Podman["Podman rootless"]
    Systemd["systemd / Quadlet"]
    Caddy["Caddy"]
    App["Aplicação containerizada"]
    DNS["DNS público"]
    Internet["Cliente na internet"]
    FS["Sistema de arquivos controlado"]
    Legacy["Fluxo anterior de contingência"]

    Operator -->|"SSH: CLI / TUI"| Pneuma
    Pneuma -->|"clone, fetch, checkout"| Repo
    Pneuma -->|"estado desejado, catálogo e histórico"| SQLite
    Pneuma -->|"build, create, inspect"| Podman
    Pneuma -->|"materializa unidade e consulta supervisão"| Systemd
    Podman -->|"executa"| App
    Systemd -->|"supervisiona"| App
    Pneuma -->|"gera, valida e solicita reload"| Caddy
    Caddy -->|"reverse proxy para endpoint loopback"| App
    Pneuma -->|"check interno e externo"| App
    Pneuma -->|"checkouts, logs e config gerada"| FS
    DNS --> Caddy
    Internet -->|"HTTPS"| Caddy
    Operator -->|"reversão emergencial"| Legacy
    Legacy -->|"restaura publicação anterior"| Caddy
```

## 4. Atores

### 4.1 Operador e desenvolvedor

Responsabilidades:

- preparar o repositório compatível;
- executar CLI ou TUI;
- selecionar a revisão;
- iniciar deployments;
- decidir exposição;
- interpretar falhas;
- executar contingência quando necessário.

O operador já está autenticado pelo acesso SSH ao host. A v0.1 não introduz autenticação própria.

### 4.2 Cliente externo

É qualquer navegador ou cliente que acessa o site público. Não interage diretamente com o Pneuma.

## 5. Sistemas externos

### 5.1 Repositório Git

Fornece:

- código-fonte;
- histórico;
- commit imutável;
- `Containerfile`;
- `pneuma.toml`.

O Pneuma não altera o repositório remoto na v0.1.

### 5.2 Podman rootless

Responsável por:

- build da imagem;
- criação de containers;
- lifecycle imediato;
- inspeção do estado;
- logs do runtime;
- isolamento básico.

O Podman é fonte de verdade para o estado imediato do container.

### 5.3 systemd e Quadlet

Responsáveis por:

- supervisão do processo;
- reinício conforme política;
- inicialização após boot;
- estado do serviço no host.

A forma exata de integração deve ser validada pelo spike. O domínio do Pneuma não depende de Quadlet diretamente.

### 5.4 Caddy

Responsável por:

- TLS;
- terminação HTTP;
- reverse proxy;
- exposição pública.

O Pneuma mantém a decisão de exposição e materializa um fragmento de configuração controlado. O Caddy continua sendo fonte de verdade operacional sobre a configuração carregada.

### 5.5 SQLite

Armazena:

- catálogo de aplicações;
- configuração registrada;
- revisões conhecidas;
- deployments;
- instâncias;
- exposição desejada;
- operações e histórico.

O SQLite não é fonte de verdade para o estado observado dos processos.

### 5.6 Sistema de arquivos

Armazena:

- checkouts isolados;
- banco e backups;
- logs controlados;
- configuração gerada;
- arquivos temporários;
- metadados de instalação.

Todos os caminhos devem permanecer dentro de raízes administradas pelo Pneuma.

### 5.7 DNS

Direciona o domínio para a VPS. O gerenciamento de DNS está fora da v0.1.

### 5.8 Fluxo anterior de contingência

É o mecanismo manual ou configuração anterior capaz de restaurar o site quando o Pneuma estiver indisponível.

Ele deve permanecer documentado e testado durante a adoção da v0.1.

## 6. Cenário de implantação no host

```mermaid
flowchart TB
    subgraph Host["VPS Linux"]
        User["Sessão SSH do operador"]
        CLI["pneuma CLI"]
        TUI["pneuma TUI"]
        Core["Core e casos de uso"]
        DB[("SQLite")]
        GitAdapter["Adapter Git"]
        PodmanAdapter["Adapter Podman"]
        SystemdAdapter["Adapter systemd"]
        CaddyAdapter["Adapter Caddy"]
        Health["Health checker"]
        RepoCache["Checkouts"]
        Current["Runtime Current\n127.0.0.1:18xxx"]
        Candidate["Runtime Candidate\n127.0.0.1:18yyy"]
        CaddyProc["Caddy :443"]

        User --> CLI
        User --> TUI
        CLI --> Core
        TUI --> Core
        Core --> DB
        Core --> GitAdapter --> RepoCache
        Core --> PodmanAdapter --> Current
        Core --> PodmanAdapter --> Candidate
        Core --> SystemdAdapter
        Core --> CaddyAdapter --> CaddyProc
        Core --> Health
        Health --> Current
        Health --> Candidate
        CaddyProc --> Current
    end

    GitRemote["Git remoto"] --> GitAdapter
    Browser["Internet"] --> CaddyProc
```

Na v0.1 não existe obrigatoriamente um daemon central. Cada execução da CLI ou TUI compõe o Core e os adapters no processo local. Operações mutáveis devem obter locks antes de alterar estado.

## 7. Interações principais

| Interação | Origem | Destino | Dados |
|---|---|---|---|
| Importar aplicação | Operador | Pneuma | URL/path do repositório |
| Resolver revisão | Pneuma | Git | branch, tag ou SHA de entrada |
| Persistir catálogo | Pneuma | SQLite | aplicação e especificações |
| Construir imagem | Pneuma | Podman | checkout e build spec |
| Criar runtime | Pneuma | Podman/systemd | imagem, porta e política |
| Observar runtime | Pneuma | Podman/systemd | identificador externo |
| Verificar saúde | Pneuma | Aplicação | HTTP local/externo |
| Publicar rota | Pneuma | Caddy | domínio e upstream |
| Consultar status | Operador | Pneuma | nome/ID da aplicação |
| Restaurar contingência | Operador | Fluxo anterior | configuração manual |

## 8. Fontes de verdade

| Informação | Fonte de verdade | Cópia ou projeção |
|---|---|---|
| Código de uma revisão | Git commit | checkout local |
| Manifesto importado | registro no SQLite após validação | arquivo no repositório |
| Estado desejado do runtime | SQLite | TUI/CLI |
| Estado observado do container | Podman/systemd | última observação no SQLite |
| Deployment ativo | SQLite após ativação confirmada | papel do runtime |
| Endpoint local | runtime + alocação persistida | configuração do Caddy |
| Exposição desejada | SQLite | TUI/CLI |
| Configuração materializada | arquivo e estado carregado no Caddy | hash/estado no SQLite |
| Saúde atual | resultado do health check | último resultado persistido |
| Domínio e DNS | configuração externa | manifesto/registro |

## 9. Trust boundaries

### TB-1 — Entrada do operador

Mesmo sendo operador confiável, argumentos e paths devem ser validados para evitar erros destrutivos.

### TB-2 — Repositório importado

O repositório é conteúdo potencialmente hostil. `Containerfile`, symlinks e caminhos não recebem confiança implícita.

### TB-3 — Container

A aplicação é isolada do host. Não recebe modo privilegiado, mounts arbitrários ou socket do Podman.

### TB-4 — Alteração do Caddy

A passagem de configuração gerada para o Caddy cruza uma fronteira de privilégio e precisa ser mínima, validada e auditável.

### TB-5 — Internet

Somente o Caddy é exposto publicamente. Runtime, SQLite, CLI, TUI e interfaces internas não devem ficar acessíveis pela internet.

## 10. Fluxos de contexto

### 10.1 Importação

```mermaid
sequenceDiagram
    actor O as Operador
    participant P as Pneuma
    participant G as Git/Filesystem
    participant M as Manifest parser
    participant D as SQLite

    O->>P: app import <source>
    P->>G: preparar origem
    G-->>P: checkout inicial
    P->>M: carregar pneuma.toml
    M-->>P: especificação validada
    P->>D: inserir aplicação e specs
    D-->>P: application_id
    P-->>O: Registered / Not deployed
```

### 10.2 Deployment

```mermaid
sequenceDiagram
    actor O as Operador
    participant P as Pneuma
    participant D as SQLite
    participant G as Git
    participant R as Podman/systemd
    participant H as Health checker
    participant C as Caddy

    O->>P: deploy application revision
    P->>D: criar Deployment(Pending)
    P->>G: resolver commit e checkout
    P->>D: estado Building
    P->>R: build da imagem
    P->>R: criar Candidate em nova porta
    P->>D: estado Verifying
    P->>H: health check interno
    alt candidato saudável
        P->>C: validar e trocar upstream
        P->>H: health check externo
        alt endpoint externo saudável
            P->>D: marcar Candidate como Current
            P->>R: parar Current anterior
            P->>D: Deployment Succeeded
            P-->>O: sucesso
        else falha externa
            P->>C: restaurar upstream anterior
            P->>R: remover Candidate
            P->>D: Deployment RolledBack/Failed
            P-->>O: falha com anterior preservada
        end
    else candidato inválido
        P->>R: remover Candidate
        P->>D: Deployment Failed
        P-->>O: falha com anterior preservada
    end
```

## 11. Eventos externos fora de controle

O design deve considerar:

- repositório indisponível;
- perda de rede durante fetch;
- falta de espaço;
- Podman indisponível;
- reboot do host;
- Caddy inválido ou indisponível;
- DNS ainda apontando para configuração anterior;
- container removido manualmente;
- banco restaurado para snapshot antigo.

Esses eventos não são evitados pelo Pneuma; devem ser detectados e diagnosticados.

## 12. Decisões não tomadas neste documento

Este documento não decide:

- schema SQL detalhado;
- assinatura final das traits;
- crates físicos;
- biblioteca Rust de SQLite;
- biblioteca HTTP;
- formato visual da TUI;
- política final de retenção;
- mecanismo exato de elevação mínima para reload do Caddy.

Essas decisões pertencem à arquitetura detalhada, ADRs ou spikes.
