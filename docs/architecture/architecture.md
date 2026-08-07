# Arquitetura do Pneuma

**Status:** documento vivo — descreve o sistema como implementado.

## 1. Estrutura

Crate único organizado em três camadas:

- `src/main.rs` — CLI fina com parsing manual de argumentos (sem clap); compõe
  configuração e chama os casos de uso; não contém lógica de domínio.
- `src/domain/` — tipos de domínio puros (`application`, `manifest`), sem
  dependências externas.
- `src/use_cases/` — casos de uso que orquestram adapters e domínio
  (`application_import`, `application_list`, `application_runtime`,
  `deployment_create`, `deployment_deploy_internal`, `deployment_list`,
  `deployment_promote_internal`, `deployment_promote_public`,
  `deployment_register_runtime`, `deployment_transition`).
  `deployment_deploy_internal` orquestra o deployment inteiro, interno e
  público, emitindo progresso passo a passo. `deployment_transition` aplica a
  máquina de estados persistida.
- `src/adapters/` — integrações com sistemas externos (`git_source`,
  `local_build`, `local_runtime`, `caddy_exposure`, `external_health`,
  `health_check`, `database`).

Sem traits, generics, macros ou async: as restrições de
[`docs/rust-guidelines.md`](../rust-guidelines.md) valem para toda mudança.

## 2. Efeitos externos

Toda integração é um processo filho com argumentos estruturados, sem shell:
`git`, `podman` (rootless), `caddy`, `curl`. Não existe daemon; cada execução
da CLI compõe tudo no processo local e termina.

## 3. Persistência

SQLite (rusqlite bundled) é a única persistência. Migrations versionadas e
imutáveis vivem em `migrations/` e são registradas via `include_str!` em
`src/adapters/database.rs`, que aplica as pendentes em cada abertura de conexão
(`PRAGMA foreign_keys = ON`).

Regras observadas:

- transações curtas, nunca abertas durante Git, build, Podman, Caddy ou HTTP;
- intenção persistida antes dos efeitos; conclusão persistida após confirmar o
  efeito (saga local, sem transação distribuída);
- a promoção pública (runtime `current`, deployment `succeeded` e exposição
  `active`) acontece em uma única transação;
- o banco não é fonte do estado observado do runtime; o Podman é.

Todos os paths vêm de variáveis de ambiente (`PNEUMA_DATABASE_PATH`,
`PNEUMA_WORKSPACE_PATH`, `PNEUMA_CADDY_MANAGED_PATH`, `PNEUMA_CADDYFILE_PATH`),
com defaults em `/var/lib/pneuma` e `/etc/caddy`.

## 4. Runtime

- containers nomeados `pneuma-<aplicação>-<commit>` com labels de aplicação,
  revisão e papel;
- publicação restrita a loopback: `127.0.0.1::<container_port>`, com a porta do
  host escolhida pelo Podman — a candidata nunca é alcançável publicamente;
- sem modo privilegiado, mounts arbitrários ou acesso ao socket do Podman;
- nomes determinísticos permitem reconciliar um redeployment da mesma revisão
  sem rebuild.

A supervisão por systemd/Quadlet prevista na D0 não foi implementada; o runtime
é um container Podman direto.

### 4.1 Ciclo de vida do runtime

- a promoção do deployment marca `applications.desired_runtime_state` como
  `running`, persistindo a intenção junto da ativação;
- `app status` observa o container atual (`role = 'current'`) no Podman e
  registra a observação: `last_observed_state`, `last_observed_at` e, quando em
  execução, `host_port`; se o container estiver ausente, persiste
  `missing`/`removed_at` e orienta um novo deployment;
- `app stop` e `app start` persistem o estado desejado antes do efeito externo,
  controlam o container e persistem a observação resultante (saga local);
- parar uma aplicação já parada e iniciar uma já em execução são sucessos
  idempotentes;
- aplicação registrada mas nunca implantada, e nome desconhecido, falham antes
  de qualquer efeito externo.

## 5. Exposição pelo Caddy

Aplicações públicas são publicadas por fragmentos `<application-id>.caddy` no
diretório gerenciado, importado pelo `Caddyfile` principal:

1. gerar o fragmento em arquivo temporário na mesma filesystem;
2. `caddy validate` contra o `Caddyfile` completo;
3. rename atômico e `caddy reload`;
4. health check externo;
5. falha externa restaura o fragmento anterior e recarrega; se a recuperação
   falhar, a exposição fica `diverged` para inspeção manual.

## 6. Health check

- **interno:** HTTP no endpoint loopback da candidata, antes de qualquer troca
  de tráfego;
- **externo (público):** `curl` em `https://<domínio><path>` com
  `--resolve <domínio>:443:127.0.0.1`, verificando o listener local do Caddy
  com retries.

## 7. Máquina de estados do deployment

```mermaid
stateDiagram-v2
    [*] --> Pending
    Pending --> PreparingSource
    PreparingSource --> Building
    Building --> Starting
    Starting --> VerifyingInternal
    VerifyingInternal --> Succeeded : aplicação interna
    VerifyingInternal --> SwitchingTraffic : aplicação pública
    SwitchingTraffic --> VerifyingExternal
    VerifyingExternal --> Succeeded

    Pending --> Failed
    PreparingSource --> Failed
    Building --> Failed
    Starting --> Failed
    VerifyingInternal --> Failed
    SwitchingTraffic --> Failed
    VerifyingExternal --> Failed
```

Todo `Failed` persiste código, estágio e mensagem; a candidata é removida e a
revisão anterior (rota e runtime) é preservada. Apenas um deployment ativo por
aplicação é permitido (`create_deployment`).
