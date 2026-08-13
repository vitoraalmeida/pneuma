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
- Registry local (container `registry:2` na porta 5000) para entregar as
  fixtures por digest (seção 6.2).

> **Nota:** a VM usa rede NAT do libvirt com DHCP; o IP pode mudar entre
> restores de snapshot. Ao reconectar depois de um restore, confira o IP atual
> (`virsh -c qemu:///system domifaddr <vm>`) e atualize `~/.ssh/config`.

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

Com a chave de provisionamento já instalada, envie o script e a biblioteca
comum e execute como root na VM. O layout no `/tmp` deve preservar a estrutura
do repositório (`provision-host.sh` em `dev-vm/`, biblioteca em `lib/`), porque
o script calcula o caminho da biblioteca a partir do próprio caminho:

```bash
scp scripts/dev-vm/provision-host.sh pneuma-dev:/tmp/dev-vm/
scp -r scripts/lib pneuma-dev:/tmp/
ssh pneuma-dev 'sudo bash /tmp/dev-vm/provision-host.sh'
```

A VM e a VPS aplicam **as mesmas invariantes de host**, implementadas uma única
vez em `scripts/lib/provision-host.sh` e usadas também por
`scripts/bootstrap-vps.sh`. O script assume uma VM Debian básica e:

1. instala o conjunto de runtime (Podman, `uidmap`, `fuse-overlayfs`, Caddy,
   Git e `curl`) e, como conveniência exclusiva da VM, `sqlite3`;
2. verifica o gerador Quadlet (`podman-user-generator` >= 4.4);
3. cria o usuário `pneuma` com `subuid/subgid` e linger;
4. cria os diretórios persistentes do Pneuma com as permissões da VPS;
5. configura o Caddyfile para importar apenas `/etc/caddy/applications/*.caddy`;
6. grava o ambiente canônico em `/etc/pneuma/environment` e as variáveis
   `PNEUMA_*`/rootless no `~/.profile` do `pneuma`;
7. valida `caddy validate` e inicia o serviço;
8. confirma Podman rootless com `podman info`.

Ao contrário do bootstrap de produção, o provisionamento da VM **não** clona o
repositório, não compila nem instala o binário, não instala a chave CI nem roda
`pneuma doctor`: o acesso de provisionamento é pré-existente e a instalação do
Pneuma é um passo separado (seção 4).

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
Containerfile.

### 6.1. Copiar e importar

Copie as fixtures para o checkout na VM (owner `pneuma:pneuma`) e registre-as
por **Git remoto** (a v0.2 removeu o import por path local): para fixtures
locais, crie um repositório Git acessível pela MV e importe pela URL.

```bash
scp -r scripts/dev-vm/fixtures pneuma-dev:/var/lib/pneuma/checkouts/
ssh pneuma-dev 'chown -R pneuma:pneuma /var/lib/pneuma/checkouts/fixtures'
# Dentro da VM, torne o diretório das fixtures um repositório Git remoto:
ssh pneuma-dev 'su - pneuma -c "
  cd /var/lib/pneuma/checkouts/fixtures/healthy-http &&
  git init -q && git add . && git -c user.email=dev@local -c user.name=dev commit -qm initial && 
  git clone --bare . /var/lib/pneuma/checkouts/healthy-http.git"'
ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app import file:///var/lib/pneuma/checkouts/healthy-http.git --manifest pneuma.toml"'
```

O script `deploy-all-fixtures.sh` automatiza esse processo para todas as
fixtures: cria um repositório Git local por fixture em
`/var/lib/pneuma/repos/<fixture>.git` e importa via `file://`.

> **Atenção:** `app import` usa `ON CONFLICT(name) DO NOTHING`; um re-import após
> alterar `pneuma.toml` **não** atualiza a entrega registrada. Para trocar o
> repositório/entrega de uma fixture já registrada, atualize o banco:
>
> ```bash
> runuser -u pneuma -- bash -lc 'cd $HOME && sqlite3 /var/lib/pneuma/database/pneuma.sqlite3 \
>   "UPDATE application_delivery_specs SET image_repository = replace(image_repository, \
>   '\''localhost/'\'', '\''localhost:5000/'\'')"'
> ```

### 6.2. Registry local e deploy por digest

As fixtures são construídas e publicadas num registry local (container
`registry:2`, porta 5000). **O digest usado no deploy é o do manifest no
registry, não o Image ID local** — o push reescreve o manifest para OCI. Para
obter o digest do registry:

```bash
curl -s -H "Accept: application/vnd.oci.image.manifest.v1+json" \
  http://localhost:5000/v2/<fixture>/manifests/latest -D - -o /dev/null \
  | grep -i docker-content-digest
```

Configure o registry como inseguro em `/etc/containers/registries.conf.d/pneuma-dev.conf`
(formato v2; o antigo `[registries.insecure]` é rejeitado):

```text
[[registry]]
location = "localhost:5000"
insecure = true
```

Construa, publique e deploye:

```bash
podman build -t localhost:5000/<fixture>:latest /var/lib/pneuma/checkouts/fixtures/<fixture>
podman push --tls-verify=false localhost:5000/<fixture>:latest
pneuma app deploy <fixture> --image localhost:5000/<fixture>@sha256:<digest-do-registry>
```

> **Enforcement de repositório:** o deploy só aceita imagens cujo repositório
> (`localhost:5000/<fixture>`) bate com `[delivery] image` do `pneuma.toml`.
> O argumento `--image` aceita apenas `<repository>@sha256:<hex>` (o digest
> solto é rejeitado).

### 6.3. Resultado esperado da bateria

| Fixture | Deploy | Observação |
|---|---|---|
| `healthy-http` | Succeeded | Porta host alocada; `/` responde a versão |
| `unhealthy-http` | Failed | Health check recebe 500 |
| `slow-start` | Failed | Health 503 dentro da janela de verificação |
| `bad-port` | Failed | Conexão recusada (porta divergente) |
| `redirect-public` | Succeeded | Requer Caddy com `local_certs` (seção 7) |

Upgrade e rollback usam um digest novo/antigo do mesmo repositório; a cada
deploy o runtime anterior é retirado e o novo ganha uma porta host nova.

### 6.4. Scripts de automação do ciclo

Os scripts em `scripts/dev-vm/` automatizam o ciclo de desenvolvimento contra a
VM (todos aceitam `[ssh-host]` opcional, default `pneuma-dev`):

Os scripts que alteram Caddy, diretórios de estado, a instalação do binário ou
reiniciam a VM esperam que o alias SSH conecte como `root`; eles não exigem nem
instalam `sudo`. Os comandos de runtime continuam sob o usuário `pneuma`.

| Script | O que faz | Quando usar |
|---|---|---|
| `sync-binary.sh` | `cargo build --release` + scp + install + `pneuma doctor` | Após alterar código Rust |
| `rebuild-fixtures.sh` | Copia fixtures, build + push no registry local, mostra digests | Após editar fixtures/`server.py` |
| `deploy-all-fixtures.sh` | Cria repos Git locais, importa cada fixture por `file://` e deploya por digest | Após reset ou mudança de fixtures |
| `reset-fixtures.sh` | Para apps, remove units/containers/Caddy fragments/checkouts, recria o DB | Voltar a um estado limpo |
| `overview.sh` | Status de apps, containers, units, Caddy e registry de uma vez | Debug rápido |
| `e2e.sh` | Reset → rebuild → deploy → verifica health → upgrade → rollback → reboot → verifica | Bateria completa de regressão |
| `test-branch-deploy.sh` | Cria repo Git com `main`/`staging`, taggeia imagens com o SHA de cada commit, importa por URL Git e deploya por `--branch` | Validar o fluxo Git → OCI (fase G) |

Fluxo típico de desenvolvimento:

```bash
scripts/dev-vm/sync-binary.sh        # depois de cada mudança de código
scripts/dev-vm/overview.sh           # inspecionar o estado
```

Fluxo de reset completo:

```bash
scripts/dev-vm/reset-fixtures.sh
scripts/dev-vm/rebuild-fixtures.sh
scripts/dev-vm/deploy-all-fixtures.sh
```

> **Nota:** `e2e.sh` reinicia a VM (`sudo reboot`) e espera ela voltar; não o
> execute durante trabalho não persistido na VM. O `reset-fixtures.sh` apaga o
> banco e os checkouts — a VM volta ao estado pós-provisionamento.

## 7. DNS local e Caddy

O provisionamento da VM configura `local_certs`, mapeia
`redirect-public.pneuma.test` para `127.0.0.1` em `/etc/hosts`, instala a CA
local do Caddy no trust store e executa `update-ca-certificates`. O E2E exige o
redirect HTTPS desse fixture e a transição posterior para `internal`; não há
skip de TLS local.

Para testar nomes adicionais sem DNS público, adicione-os ao `/etc/hosts` da
VM:

```text
192.168.122.50 site.pneuma.test
192.168.122.50 api.pneuma.test
```

Aplicações públicas passam por **external health check via HTTPS**; sem um
domínio real a VM usa certificados locais. A configuração equivalente é:

```caddy
{
    local_certs
}
```

```bash
# CA raiz local do Caddy (instalada automaticamente pelo provisionamento)
sudo cp /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt \
  /usr/local/share/ca-certificates/caddy-local-root.crt
sudo update-ca-certificates
```

Sem isso, o health check externo de uma app `public` falha com erro de TLS e o
deploy é marcado Failed (em produção não é necessário: Let's Encrypt emite o
certificado real).

## 8. Snapshots e reset

Crie pelo menos dois snapshots via `virt-manager` ou `virsh`
(`-c qemu:///system`):

| Snapshot | Estado |
|---|---|
| `pneuma-dev-base` | Podman/Caddy/user/diretórios prontos, Pneuma instalado |
| `pneuma-dev-fixtures-ready` | Fixtures registradas, registry local, Caddy `local_certs`, baseline E2E |

Testes destrutivos (rollback, reboot, recovery, Caddy quebrado, banco
inconsistente) devem começar de `pneuma-dev-base`, sem acumular estado invisível
entre execuções.

> **Nota:** a VM usa DHCP do libvirt; após restaurar um snapshot o IP pode
> mudar (o atual está em `~/.ssh/config`). Não confie no IP antigo.

## 9. Segurança do ambiente

- Usar chave SSH exclusiva para a VM.
- Não copiar secrets de produção para a VM.
- Usar registry público para fixtures ou credencial read-only exclusiva.
- Não expor o SSH da VM à Internet (rede NAT/libvirt).
- Executar o Pneuma como usuário não-root (`pneuma`).
- Restringir root ao provisionamento e à instalação do binário.
- Bloquear login por senha do usuário `pneuma` (`passwd -l`).
- A chave CI usa `restrict` e forced command; o E2E exige apenas `version` e
  `deploy healthy-http staging`, e rejeita shell, PTY, forwarding, agent/X11
  forwarding, leitura de arquivo e injection em branch.

## 10. Próximos passos

A bateria `scripts/dev-vm/e2e.sh` já cobre o ciclo principal (import, deploy por
digest, upgrade, rollback e reboot). Upgrade/rollback e reboot foram validados
na VM: o Quadlet (via `[Install] WantedBy=default.target`) restaura as
aplicações no boot com linger habilitado, sem `systemctl enable` explícito.
Com a v0.2 concluída, o fluxo Git → OCI é coberto por `test-branch-deploy.sh`
(repo Git com `main`/`staging`, import por URL `file://` e deploy por `--branch`)
e o `e2e.sh` importa as fixtures por repositórios Git locais. A VPS é usada
apenas para smoke final de integração pública (DNS e TLS reais).

## Referências

- `scripts/dev-vm/provision-host.sh` — provisionamento do host.
- `scripts/dev-vm/smoke.sh` — verificação básica (version, doctor, app list).
- `scripts/dev-vm/sync-binary.sh` — build + deploy do binário na VM.
- `scripts/dev-vm/{rebuild,deploy-all,reset,overview,e2e}.sh` — automação do
  ciclo de fixtures (seção 6.4).
- `scripts/dev-vm/fixtures/` — cinco fixtures determinísticas para os cenários
  E2E (seção 6).
