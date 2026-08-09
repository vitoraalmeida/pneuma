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

- VM Debian 13 (trixie) **básica** já criada e acessível por SSH a partir do
  host de desenvolvimento — Debian 12 (bookworm) entrega Podman 4.3.1, que
  **não inclui** o gerador Quadlet (`podman-user-generator`).
- Chave SSH **exclusiva** para a VM, gerada no host antes do provisionamento
  (seção 2).
- Aplicações fixture pequenas e determinísticas para os testes (seção 6).

> **Nota:** uma conta com acesso root (por exemplo `root` ou um usuário com
> `sudo`) serve apenas para o provisionamento. O Pneuma roda sob o usuário
> dedicado `pneuma`, sem acesso ao root, replicando o modelo da VPS.

## 2. Gerar a chave SSH e configurar o acesso

Gere uma chave exclusiva para a VM no host de desenvolvimento (conforme o
plano, o usuário `pneuma` não usa a chave do GitHub):

```bash
ssh-keygen -t ed25519 -f ~/.ssh/pneuma-dev -N "" -C "pneuma-dev VM key"
```

Copie a chave pública para uma conta de provisionamento da VM (root ou
administrativa). Com `ssh-copy-id`, informando o IP que a VM recebeu na rede
(ex.: `192.168.122.50`):

```bash
ssh-copy-id -i ~/.ssh/pneuma-dev.pub root@192.168.122.50
```

O Debian aceita login root por chave (`PermitRootLogin prohibit-password`) sem
expor a senha. Configure o `~/.ssh/config` do host para acesso previsível:

```text
Host pneuma-dev
    HostName 192.168.122.50
    User root
    IdentityFile ~/.ssh/pneuma-dev
    IdentitiesOnly yes
```

E adicione ao `/etc/hosts` do host: `192.168.122.50 pneuma-dev`. Confirme:

```bash
ssh pneuma-dev 'hostname'
```

## 3. Provisionar o host

Com a chave de provisionamento já instalada, envie o script e execute como
root na VM:

```bash
scp scripts/dev-vm/provision-host.sh pneuma-dev:/tmp/
ssh pneuma-dev 'sudo bash /tmp/provision-host.sh'
```

O script assume uma VM Debian básica e:

1. instala Podman, `uidmap`, `fuse-overlayfs`, Caddy, Git, `sqlite3` e `curl`;
2. verifica o gerador Quadlet (`podman-user-generator` >= 4.4);
3. cria o usuário `pneuma` com `subuid/subgid` e linger;
4. cria os diretórios persistentes do Pneuma com as permissões da VPS;
5. configura o Caddyfile para importar apenas `/etc/caddy/applications/*.caddy`;
6. grava as variáveis `PNEUMA_*` no `~/.profile` do `pneuma`;
7. valida `caddy validate` e inicia o serviço;
8. confirma Podman rootless com `podman info`.

O script **não** instala a chave SSH nem o binário: o acesso de provisionamento
é pré-existente e a instalação do Pneuma é um passo separado (seção 4).

## 4. Instalar o binário Pneuma na VM

A VM não compila nem clona o repositório. O ciclo parte do binário compilado no
host, e a instalação é um passo **separado** do provisionamento: envie o
binário e instale em `/usr/local/bin/pneuma`:

```bash
cargo build --release
scp target/release/pneuma pneuma-dev:/tmp/pneuma-new
ssh pneuma-dev 'sudo install -o root -g root -m 0755 /tmp/pneuma-new /usr/local/bin/pneuma'
```

Valide o binário antes de instalar e rode `pneuma doctor` como o usuário
`pneuma` depois, para que uma build quebrada nunca substitua um runtime
funcionando:

```bash
ssh pneuma-dev '/usr/local/bin/pneuma version'
ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma doctor"'
```

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
cargo build --release
    ↓
scp target/release/pneuma pneuma-dev:/tmp/pneuma-new
    ↓
ssh pneuma-dev 'sudo install ... /usr/local/bin/pneuma'
    ↓
ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd $HOME && pneuma doctor"'
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
- `scripts/dev-vm/smoke.sh` — verificação básica (version, doctor, app list).
- [`vps-bootstrap.md`](vps-bootstrap.md) — bootstrap da VPS de produção.
- [`public-deployment.md`](public-deployment.md) — deployment público.
