# VM de desenvolvimento Pneuma (Debian 13)

Tutorial para criar e preparar uma VM Debian 13 que reproduz as propriedades
relevantes da VPS de produção e serve como alvo padrão de integração e testes
E2E do Pneuma — sem usar a VPS como laboratório. O plano completo está em
`~/Downloads/pneuma-development-vm-plan.md`; este documento é o passo a passo
operacional.

A VM é o **host operacional** do Pneuma, não uma segunda estação de
desenvolvimento: a edição, compilação e os testes unitários continuam no host.
A VM valida Podman rootless, Caddy, Quadlet/systemd, SQLite, permissões,
networking, instalação do binário e reboot/recovery.

## 1. Pré-requisitos

- VM Debian 13 (trixie) já criada e acessível por SSH a partir do host de
  desenvolvimento — Debian 12 (bookworm) entrega Podman 4.3.1, que **não
  inclui** o gerador Quadlet (`podman-user-generator`).
- Host de desenvolvimento Linux com acesso SSH por chave para a VM.
- Aplicações fixture pequenas e determinísticas para os testes (seção 7).

> **Nota:** uma conta administrativa (por exemplo `dev`) com `sudo` serve
> apenas para o provisionamento. O Pneuma roda sob o usuário dedicado `pneuma`,
> sem acesso ao root, replicando o modelo da VPS.

## 2. Configurar o acesso SSH

Confirme o SSH e copie a chave pública exclusiva da VM:

```bash
ssh-copy-id dev@pneuma-dev
ssh pneuma-dev
```

Para um endereço previsível, adicione ao `/etc/hosts` do host de
desenvolvimento o IP que a VM recebeu na rede (ex.: `192.168.122.50 pneuma-dev`).

## 3. Provisionar o host

Envie o script de provisionamento para a VM e execute como root:

```bash
scp scripts/dev-vm/provision-host.sh pneuma-dev:/tmp/
ssh pneuma-dev 'sudo bash /tmp/provision-host.sh'
```

O script:

1. instala Podman, `uidmap`, `fuse-overlayfs`, Caddy, Git, `sqlite3` e `curl`;
2. verifica o gerador Quadlet (`podman-user-generator` >= 4.4);
3. cria o usuário `pneuma` com `subuid/subgid` e linger;
4. cria os diretórios persistentes do Pneuma com as permissões da VPS;
5. configura o Caddyfile para importar apenas `/etc/caddy/applications/*.caddy`;
6. grava as variáveis `PNEUMA_*` no `~/.profile` do `pneuma`;
7. valida `caddy validate` e inicia o serviço;
8. confirma Podman rootless com `podman info`.

Se quiser que o script já instale a chave pública SSH do host de desenvolvimento
no usuário `pneuma`:

```bash
ssh pneuma-dev 'sudo bash /tmp/provision-host.sh /home/dev/.ssh/pneuma-dev.pub'
```

## 4. Instalar o binário Pneuma na VM

A VM não compila nem clona o repositório. O ciclo parte do binário compilado no
host. Há dois caminhos:

### Caminho rápido (recomendado)

Do host de desenvolvimento, dentro do repositório do Pneuma:

```bash
./scripts/dev-vm/deploy.sh
```

O script executa `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test
--all-features` e `cargo build --release`; envia `target/release/pneuma` por
SCP para `/tmp/pneuma-new`; valida o binário na VM; instala em
`/usr/local/bin/pneuma`; e executa `pneuma doctor`. Falha imediatamente em
qualquer erro.

### Caminho manual

Se o binário já foi compilado:

```bash
scp target/release/pneuma pneuma-dev:/tmp/pneuma-new
ssh pneuma-dev 'sudo bash /tmp/install-pneuma.sh /tmp/pneuma-new'
```

O `install-pneuma.sh` valida o binário (version + doctor) antes e depois de
instalar, para que uma build quebrada nunca substitua um runtime funcionando.

## 5. Verificação

Na VM, abra o shell do `pneuma` e confirme o ambiente:

```bash
sudo -iu pneuma
pneuma version
pneuma doctor
pneuma app list
```

A VM recém-provisionada ainda não tem aplicações registradas; o `pneuma app
list` deve retornar uma lista vazia (ou a mensagem correspondente), e o
`pneuma doctor` deve passar em todos os checks de host.

O ciclo normal de desenvolvimento passa a ser:

```text
editar código
    ↓
./scripts/dev-vm/deploy.sh
    ↓
Pneuma atualizado e validado na VM
```

## 6. Aplicações fixture

Manter as fixtures independentes do site pessoal, pequenas e determinísticas:

| Fixture | Comportamento | Uso |
|---|---|---|
| `healthy-http` | `/healthz` 200; `/` mostra versão | Happy path, upgrade, rollback |
| `unhealthy-http` | `/healthz` 500 | Preservação da release ativa |
| `slow-start` | Health 200 após atraso controlado | Janela de verificação |
| `bad-port` | Porta diferente da declarada | Falha de runtime/configuração |
| `redirect-public` | HTTP simples atrás do Caddy | Visibility e proxy |

Cada fixture vive em `scripts/dev-vm/fixtures/<nome>/` com seu `pneuma.toml` e
Containerfile. Para registrar e deployar uma fixture no fluxo de integração:

```bash
pneuma app import /var/lib/pneuma/checkouts/<fixture>
pneuma app deploy <fixture-app> --image <registry>/<fixture>@sha256:<digest>
```

## 7. DNS local e Caddy

Para validar o proxy do Caddy sem DNS público, use nomes locais no `/etc/hosts`
do host:

```text
192.168.122.50 site.pneuma.test
192.168.122.50 api.pneuma.test
```

HTTP é suficiente para a maioria dos cenários; TLS público/Let's Encrypt
continua sendo validado somente na VPS.

## 8. Snapshots e reset

Crie pelo menos três snapshots via `virt-manager` ou `virsh`:

| Snapshot | Estado |
|---|---|
| `00-debian-clean` | Debian instalado, antes do provisionamento |
| `10-pneuma-host-ready` | Podman/Caddy/user/diretórios prontos |
| `20-pneuma-fixtures-ready` | Fixtures registradas e baseline E2E |

Testes destrutivos (rollback, reboot, recovery, Caddy quebrado, banco
inconsistente) devem começar de `10` ou `20`, sem acumular estado invisível
entre execuções.

## 9. Segurança do ambiente

- Usar chave SSH exclusiva para a VM.
- Não copiar secrets de produção para a VM.
- Usar registry público para fixtures ou credencial read-only exclusiva.
- Não expor o SSH da VM à Internet (rede NAT/libvirt).
- Executar o Pneuma como usuário não-root (`pneuma`).
- Restringir root ao provisionamento e à instalação do binário.
- Bloquear login por senha do usuário `pneuma` (`passwd -l`).

## 10. Próximos passos

Com a VM pronta, os cenários E2E obrigatórios (seção 9 do plano) podem ser
automatizados em `scripts/dev-vm/e2e.sh`, cobrindo import, deploy por digest,
release saudável/unhealthy, rollback, visibility, stop/start, reboot e
backup/restore. A VPS passa a ser usada apenas para smoke final de integração
pública (DNS e TLS reais).

## Referências

- `scripts/dev-vm/provision-host.sh` — provisionamento do host.
- `scripts/dev-vm/install-pneuma.sh` — instalação do binário na VM.
- `scripts/dev-vm/deploy.sh` — ciclo build + SCP + install + doctor.
- `scripts/dev-vm/smoke.sh` — verificação básica (version, doctor, app list).
- [`vps-bootstrap.md`](vps-bootstrap.md) — bootstrap da VPS de produção.
- [`public-deployment.md`](public-deployment.md) — deployment público.
