# Deployment público pela CLI

O fluxo público exige Podman rootless, Caddy e `curl` no host. O domínio já deve
apontar para a VPS, e o Caddy precisa controlar as portas 80 e 443 para obter e
servir o certificado HTTPS. O CI publica uma imagem OCI imutável; a VPS apenas
puxa e implanta seu digest.

O Caddyfile principal deve importar exclusivamente a área gerenciada pelo
Pneuma:

```caddyfile
import /etc/caddy/applications/*.caddy
```

O usuário que executa o Pneuma precisa conseguir criar e substituir arquivos em
`/etc/caddy/applications` e executar `caddy validate` e `caddy reload` contra
`/etc/caddy/Caddyfile`. Valide essas permissões antes de mover o domínio
principal.

Os paths e a faixa de portas podem ser substituídos sem alterar o manifesto:

```bash
export PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3
export PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts
export PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications
export PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile
export PNEUMA_RUNTIME_PORT_RANGE=30000-39999
export PNEUMA_QUADLET_DIR=$HOME/.config/containers/systemd
```

Depois que a imagem passar no CI e estiver publicada no registry, importe o
checkout que contém o manifesto v2 e implante o digest exato:

```bash
pneuma app import /srv/vitoralmeida.tech
pneuma --verbose app deploy vitoralmeida-tech-prod \
  --image ghcr.io/owner/vitoralmeida-tech@sha256:<digest>
```

O repositório da imagem precisa coincidir com `[delivery] image` no
`pneuma.toml`; tags mutáveis são rejeitadas. `app deploy-source` permanece como
caminho alternativo para build local, mas não é o fluxo público padrão.

O deployment somente termina como `Succeeded` depois de:

1. reservar uma porta loopback fixa e iniciar a candidata por uma unidade
   Quadlet do deployment;
2. verificar o health check pelo endpoint loopback;
3. validar e recarregar a rota do Caddy;
4. acessar `https://<domain><health-path>` pelo listener local do Caddy;
5. promover runtime, deployment e exposição em uma transação SQLite;
6. habilitar a unidade Quadlet promovida e tentar retirar o runtime anterior.

Falha depois da troca do Caddy restaura e recarrega o fragmento anterior antes
de remover a candidata. Se a própria recuperação falhar, o erro é preservado e
a exposição fica marcada como `diverged`, exigindo inspeção manual antes de nova
promoção.

## Validação operacional

Após o primeiro deployment Quadlet, confirme que a unidade está habilitada e
sobrevive a reboot:

```bash
ls -l "$XDG_RUNTIME_DIR/systemd/generator/default.target.wants/pneuma-<app>-*.service"
systemctl --user is-active pneuma-<app>-*.service
pneuma app status <app>
pneuma doctor
```

Unidades Quadlet geradas com `[Install] WantedBy=default.target` aparecem
como `generated` em `systemctl --user is-enabled`; a verificação correta é o
symlink do gerador acima.

Depois de reiniciar a VPS, `app status` deve observar o runtime como `Running` e
o endpoint público deve retornar o status esperado. `pneuma app start` e
`pneuma app stop` habilitam/iniciam ou param/desabilitam a unidade; runtimes
anteriores ao Quadlet continuam usando o fallback direto do Podman até serem
redeployados.

Em 7 de agosto de 2026, o fluxo de Caddy foi exercitado em produção: aplicação
`vitoralmeida-tech-prod`, domínio `vitoralmeida.tech`, repositório
`https://github.com/vitoraalmeida/vitoralmeida.tech`. O runtime roda como Podman
rootless sob o usuário `pneuma`, o Caddy controla as portas 80 e 443 com o
certificado HTTPS e o site responde `https://vitoralmeida.tech/healthz` com HTTP
200.

## Recuperação e remoção

Consulte [`backup-and-restore.md`](backup-and-restore.md) antes de restaurar o
banco. A remoção de uma aplicação não possui comando na CLI; para desregistrar,
pare e remova o container, remova o fragmento do Caddy e recarregue, e depois
apague a linha da aplicação no SQLite com `PRAGMA foreign_keys = ON` (o `DELETE`
em `applications` remove as linhas dependentes por cascata).
