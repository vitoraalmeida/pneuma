# Bootstrap de VPS limpa (Debian 13)

Procedimento executado em agosto de 2026 no host `srv655252` (Debian 13
trixie, Podman 5.4.2, Caddy 2.x) para preparar um host do zero, instalar o
Pneuma e implantar a aplicação `vitoralmeida-tech-prod` via GHCR. O script
`scripts/bootstrap-vps.sh` automatiza a maior parte, mas este documento
registra os passos reais e as decisões operacionais.

## Pré-requisitos

- VPS Debian 13 (trixie). Debian 12 (bookworm) entrega Podman 4.3.1, que **não
  inclui o gerador Quadlet** (`podman-user-generator`); o Pneuma depende dele
  para supervisionar runtimes entre reboots.
- Registros DNS A (e AAAA, se aplicável) apontando o domínio para a VPS.
- Portas TCP 80 e 443 abertas; nenhum outro serviço (nginx, apache) ocupando-as.
- Acesso de leitura ao repositório da imagem OCI no GHCR (token de pacote ou
  repositório público).
- Repositório da aplicação acessível via HTTPS (público) ou com chave de
  deploy configurada (privado).

## Pacotes do sistema

```bash
apt-get update
apt-get install -y \
    build-essential curl git pkg-config libssl-dev \
    podman uidmap fuse-overlayfs \
    caddy
```

Confirme a versão do Podman e a presença do gerador Quadlet:

```bash
podman --version
# Deve ser >= 4.4 (Debian 13 entrega 5.4.2)

ls -l /usr/lib/systemd/user-generators/podman-user-generator
# Deve existir e ser executável
```

## Usuário `pneuma` e permissões

```bash
useradd --create-home --shell /bin/bash pneuma
PNEUMA_UID="$(id -u pneuma)"

usermod --add-subuids 100000-165535 pneuma
usermod --add-subgids 100000-165535 pneuma
passwd -l pneuma
loginctl enable-linger pneuma
```

## Diretórios

```bash
install -d -o pneuma -g pneuma -m 0700 /home/pneuma/.ssh
install -d -o pneuma -g pneuma -m 0750 \
    /var/lib/pneuma/database \
    /var/lib/pneuma/checkouts \
    /home/pneuma/.config/containers/systemd
install -d -o pneuma -g caddy -m 0750 /etc/caddy/applications
```

O diretório `/etc/caddy/applications` deve ter grupo `caddy` para que o
usuário `pneuma` possa criar fragmentos e o Caddy possa lê-los.

## Toolchain Rust e build do Pneuma

```bash
sudo -iu pneuma
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env
git clone https://github.com/<USER>/pneuma.git ~/pneuma
cd ~/pneuma
cargo build --release
```

Instale o binário como root:

```bash
install -o root -g root -m 0755 \
    /home/pneuma/pneuma/target/release/pneuma \
    /usr/local/bin/pneuma
```

## Caddy

O Caddyfile principal deve importar exclusivamente a área gerenciada pelo
Pneuma:

```bash
cat > /etc/caddy/Caddyfile <<'EOF'
import /etc/caddy/applications/*.caddy
EOF

chown root:caddy /etc/caddy/Caddyfile
chmod 0644 /etc/caddy/Caddyfile

systemctl enable --now caddy
caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl restart caddy
```

## Variáveis de ambiente do usuário `pneuma`

Adicione ao `/home/pneuma/.profile`:

```bash
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3
export PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts
export PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications
export PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile
export PNEUMA_RUNTIME_PORT_RANGE=30000-39999
export PNEUMA_QUADLET_DIR=$HOME/.config/containers/systemd
```

Abra um shell como `pneuma` e confirme o Podman rootless:

```bash
sudo -iu pneuma
podman info --format '{{.Host.Security.Rootless}}'
# Deve retornar: true
```

## Implantar a aplicação via GHCR

Clone o repositório da aplicação no workspace:

```bash
pneuma app import /var/lib/pneuma/checkouts/vitoralmeida.tech
```

Implante o digest exato publicado pelo CI:

```bash
pneuma --verbose app deploy vitoralmeida-tech-prod \
  --image ghcr.io/<USER>/vitoralmeida.tech@sha256:<digest>
```

O repositório da imagem deve coincidir com `[delivery] image` no
`pneuma.toml`; tags mutáveis são rejeitadas.

## Persistência no boot

Unidades Quadlet geradas pelo Pneuma são habilitadas via `[Install]
WantedBy=default.target`. O `systemctl --user is-enabled` reporta `generated`
(não `enabled`), então a verificação correta é o symlink do gerador:

```bash
ls -l "$XDG_RUNTIME_DIR/systemd/generator/default.target.wants/pneuma-*.service"
# Deve existir após o deploy
```

## Verificação E2E após reboot

Reinicie a VPS e confirme:

```bash
sudo -iu pneuma
pneuma app status vitoralmeida-tech-prod
# "Observed state: Running"

curl -I https://vitoralmeida.tech/healthz
# HTTP 200
```

O runtime deve sobreviver ao reboot sem intervenção manual, supervisionado
pelo systemd user manager do usuário `pneuma`.

## Diagnóstico

Execute `pneuma doctor` para validar o estado do host:

```bash
pneuma doctor
```

O comando verifica: conexão e migrações do SQLite, diretórios, Caddy, Git,
Podman rootless, gerador Quadlet, imagens OCI ativas e espaço em disco.

## Referências

- [`public-deployment.md`](public-deployment.md) — fluxo de deployment público
  e pré-requisitos do Caddy.
- [`backup-and-restore.md`](backup-and-restore.md) — backup e recuperação do
  SQLite.
- `scripts/bootstrap-vps.sh` — automação do procedimento acima.
- `scripts/verify-vps.sh` — verificação pós-bootstrap e pós-deploy.
