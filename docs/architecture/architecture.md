# Arquitetura do Pneuma

**Status:** documento vivo — descreve o sistema como implementado.

## 1. Estrutura

Crate único organizado em três camadas:

- `src/main.rs` — CLI fina com parsing manual de argumentos (sem clap); compõe
  configuração e chama os casos de uso; não contém lógica de domínio.
- `src/domain/` — tipos de domínio puros (`application`, `manifest`, `release`,
  `system`), sem dependências externas.
- `src/use_cases/` — casos de uso que orquestram adapters e domínio
  (`application_import`, `application_list`, `application_runtime`,
  `deployment_create`, `deployment_deploy_oci`, `deployment_deploy_release`,
  `deployment_deploy_source`, `deployment_list`, `deployment_promote_internal`,
  `deployment_promote_public`, `deployment_register_runtime`,
  `deployment_rollback`, `deployment_transition`, `exposure_change`,
  `release_create`, `system_create`, `system_list`, `system_show`).
  `deployment_deploy_release` orquestra o deployment inteiro (runtime, health,
  Caddy e ativação) a partir de uma Release imutável; os caminhos OCI
  (`deployment_deploy_oci`) e de fonte local (`deployment_deploy_source`)
  produzem a Release e delegam a ele. `deployment_transition` aplica a máquina
  de estados persistida.
- `src/adapters/` — integrações com sistemas externos (`git_source`,
  `local_build`, `local_runtime`, `oci_image`, `port_allocator`,
  `systemd_quadlet`, `caddy_exposure`, `health_check_external`,
  `health_check_internal`, `database`).

Sem traits, generics, macros ou async: as restrições de
[`docs/rust-guidelines.md`](../rust-guidelines.md) valem para toda mudança.

> **Direção v0.2** (ver [`roadmap.md`](../roadmap.md)): o Pneuma deixa de
> construir aplicações. `deploy-source`, `deployment_deploy_source`,
> `local_build`, `[build]` e o import por path local serão removidos; o único
> artifact deployável passa a ser `image@digest` descoberto pelo CI
> (`Git branch → commit → OCI digest`), e a persistência passa a ser organizada
> em SQLite stores por capacidade. Este documento descreve a v0.1 como
> implementada.

## 2. Efeitos externos

Toda integração é um processo filho com argumentos estruturados, sem shell:
`git`, `podman` (rootless), `systemctl --user`, `caddy`, `curl` e `df`. O
Pneuma não possui daemon ou control plane próprio: cada execução da CLI compõe
tudo no processo local e termina. A supervisão persistente dos containers é do
user manager do systemd via Quadlet.

## 3. Persistência

SQLite (rusqlite bundled) é a única persistência. Migrations versionadas e
imutáveis vivem em `migrations/` e são registradas via `include_str!` em
`src/adapters/database.rs`, que aplica as pendentes em cada abertura de conexão
(`PRAGMA foreign_keys = ON`).

A especificação da aplicação é persistida na importação do `pneuma.toml`
(schema v2): `application_sources` e `application_build_specs` existem apenas
quando o manifesto declara `[source]`/`[build]` (caminho `deploy-source`), e
`application_delivery_specs` sempre guarda o repositório OCI permitido
(`[delivery] image`), usado para validar o `app deploy --image`.

Regras observadas:

- transações curtas, nunca abertas durante Git, build, Podman, Caddy ou HTTP;
- intenção persistida antes dos efeitos; conclusão persistida após confirmar o
  efeito (saga local, sem transação distribuída);
- a promoção pública (runtime do deployment ativo, deployment `succeeded` e
  exposição `active`) acontece em uma única transação;
- o banco não é fonte do estado observado do runtime; o Podman é.

`runtime_port_reservations` (migration 0012) impede que candidatas concorrentes
recebam a mesma porta loopback. A reserva existe antes de o runtime ser
registrado, é consumida após o registro e é liberada no cleanup da candidata.

Backup e restore usam a API de backup do SQLite. O restore valida
`PRAGMA integrity_check`, toma um lock exclusivo `<database>.restore.lock`,
preserva uma cópia `pre-restore`, substitui o banco por rename atômico e remove
sidecars WAL antes da próxima abertura.

Todos os paths vêm de variáveis de ambiente (`PNEUMA_DATABASE_PATH`,
`PNEUMA_WORKSPACE_PATH`, `PNEUMA_CADDY_MANAGED_PATH`, `PNEUMA_CADDYFILE_PATH`,
`PNEUMA_RUNTIME_PORT_RANGE`, `PNEUMA_QUADLET_DIR`), com defaults em
`/var/lib/pneuma`, `/etc/caddy`, `30000-39999` e
`$HOME/.config/containers/systemd`.

## 4. Runtime

- cada deployment gera uma unidade Quadlet
  `pneuma-<aplicação>-<deployment-id>.container` e container de mesmo nome,
  com labels de aplicação e revisão;
- publicação restrita a loopback:
  `127.0.0.1:<porta-reservada>:<container_port>`; a porta fixa é a menor livre
  em `PNEUMA_RUNTIME_PORT_RANGE`, e a candidata nunca é alcançável
  publicamente;
- sem modo privilegiado, mounts arbitrários ou acesso ao socket do Podman;
- a unidade tem `Restart=on-failure`; ela inicia a candidata, mas só é
  habilitada depois da promoção, portanto apenas o runtime atual volta após
  reboot.

O caminho de criação é: reservar porta → escrever a unidade → `systemctl --user
daemon-reload` → iniciar a unidade → resolver o ID do container pelo nome. A
falha em qualquer etapa limpa unidade, container, runtime candidato e reserva
quando já existirem.

Depois de uma promoção transacional bem-sucedida, o Pneuma habilita a unidade
atual e tenta retirar o runtime anterior (stop, disable, remove unit,
daemon-reload, remove container e `removed_at`). Essa finalização é best-effort:
um erro gera warning sem reverter a promoção já concluída.

### 4.1 Ciclo de vida do runtime

- a promoção do deployment marca `applications.desired_runtime_state` como
  `running`, persistindo a intenção junto da ativação;
- `app status` observa o container do deployment ativo (`active_deployment_id`)
  no Podman e registra a observação: `last_observed_state`, `last_observed_at`
  e, quando em execução, `host_port`; se o container estiver ausente, persiste
  `missing`/`removed_at` e orienta um novo deployment;
- `app stop` e `app start` persistem o estado desejado antes do efeito externo,
  controlam a unidade Quadlet e persistem a observação resultante (saga local);
  um runtime legado sem arquivo Quadlet usa `podman start`/`podman stop` até ser
  redeployado;
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
    Pending --> Starting
    Starting --> Verifying
    Verifying --> Activating : aplicação pública
    Verifying --> Succeeded : aplicação interna
    Activating --> Succeeded

    Pending --> Failed
    Starting --> Failed
    Verifying --> Failed
    Activating --> Failed
```

Todo `Failed` persiste código, estágio e mensagem; a candidata é removida e a
Release anterior (rota e runtime) é preservada. Apenas um deployment ativo por
aplicação é permitido (`create_deployment`). Rollback cria um novo deployment
(`type = rollback`) a partir da Release anterior bem-sucedida.

## 8. Operação e diagnóstico

- `pneuma database backup <path>` cria uma cópia consistente do SQLite;
  `pneuma database restore <path>` executa a recuperação descrita na seção de
  persistência antes de abrir a conexão normal da CLI;
- `pneuma doctor` verifica banco, migrations, paths, disponibilidade de Git,
  Podman e Caddy, Podman rootless funcional, validação do Caddyfile, pull das
  imagens OCI ativas e pelo menos 1 GiB livre nos filesystems do banco e do
  workspace;
- o bootstrap habilita linger para o usuário `pneuma`, permitindo que as
  unidades Quadlet user-level iniciem após reboot sem uma sessão SSH ativa.
