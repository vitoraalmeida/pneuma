# Deployment público pela CLI

O fluxo público exige `git`, Podman rootless, Caddy e `curl` no host. O domínio já deve apontar para a VPS, e o Caddy precisa controlar as portas 80 e 443 para obter e servir o certificado HTTPS.

O Caddyfile principal deve importar exclusivamente a área gerenciada pelo Pneuma:

```caddyfile
import /etc/caddy/applications/*.caddy
```

O usuário que executa o Pneuma precisa conseguir criar e substituir arquivos em `/etc/caddy/applications` e executar `caddy validate` e `caddy reload` contra `/etc/caddy/Caddyfile`. Valide essas permissões antes de mover o domínio principal.

Os paths podem ser substituídos sem alterar o manifesto:

```bash
export PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3
export PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts
export PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications
export PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile
```

Depois que o commit exato passar no CI, atualize o checkout da VPS e execute:

```bash
pneuma app import /srv/vitoralmeida.tech
pneuma --verbose app deploy vitoralmeida-tech-prod /srv/vitoralmeida.tech --revision <commit-sha>
```

O deployment somente termina como `Succeeded` depois de:

1. construir e iniciar a candidata;
2. verificar o health check pelo endpoint loopback;
3. validar e recarregar a rota do Caddy;
4. acessar `https://<domain><health-path>` pelo listener local do Caddy;
5. promover runtime, deployment e exposição em uma transação SQLite.

Falha depois da troca do Caddy restaura e recarrega o fragmento anterior antes de remover a candidata. Se a própria recuperação falhar, o erro é preservado e a exposição fica marcada como `diverged`, exigindo inspeção manual antes de nova promoção.

## Validação em produção

Em 7 de agosto de 2026, o fluxo acima foi exercitado em produção: aplicação
`vitoralmeida-tech-prod`, domínio `vitoralmeida.tech`, repositório
`https://github.com/vitoraalmeida/vitoralmeida.tech`. O runtime roda como Podman
rootless sob o usuário `pneuma`, o Caddy controla as portas 80 e 443 com o
certificado HTTPS e o site responde `https://vitoralmeida.tech/healthz` com HTTP
200.

A remoção de uma aplicação não possui comando na CLI; para desregistrar, pare e
remova o container, remova o fragmento do Caddy e recarregue, e depois apague a
linha da aplicação no SQLite com `PRAGMA foreign_keys = ON` (o `DELETE` em
`applications` remove as linhas dependentes por cascata).
