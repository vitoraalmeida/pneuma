# Validação do site no ambiente de staging

**Status:** validado manualmente em 5 de agosto de 2026  
**Hostname:** `staging.vitoralmeida.tech`  
**Objetivo:** comprovar o contrato operacional do site antes de o Pneuma assumir seu deployment

## Topologia validada

O Nginx continua responsável pelas portas públicas para não alterar a publicação
atual de `vitoralmeida.tech`. O Caddy funciona como proxy intermediário somente
para o staging:

```text
Cliente HTTP
    ↓
staging.vitoralmeida.tech
    ↓
Nginx :80
    ↓
Caddy 127.0.0.1:8081
    ↓
Container rootless 127.0.0.1:8080
```

Essa topologia é transitória. Na arquitetura final, o Caddy deverá receber o
tráfego público diretamente e administrar TLS. O Nginx permanece neste teste
para preservar o fluxo de produção existente.

## Contrato da aplicação

- imagem: `localhost/vitoralmeida.tech:test`;
- usuário do runtime no host: `pneuma`;
- usuário do processo no container: `nginx`;
- porta do container: `8080`;
- endpoint representativo: `GET /` retorna HTTP 200;
- health check: `GET /healthz` retorna HTTP 200 e o corpo `ok`;
- nenhuma porta do container é publicada em uma interface externa.

O contrato declarativo correspondente está no `pneuma.toml` do repositório do
site.

## Preparação do DNS

Foi criado um registro `A` para `staging.vitoralmeida.tech` apontando para o
IPv4 público da VPS. A resolução pode ser conferida com:

```bash
dig +short staging.vitoralmeida.tech A
```

## Transferência manual da imagem

O fluxo atual de produção transfere o diretório estático `dist/`; ele não produz
uma imagem OCI. Para esta validação, a imagem foi construída localmente e
transferida manualmente. Essa transferência não faz parte do deployment final.

Na máquina de desenvolvimento, dentro do repositório do site:

```bash
podman build -t localhost/vitoralmeida.tech:test .
podman save \
  --output /tmp/vitoralmeida-tech-test.tar \
  localhost/vitoralmeida.tech:test
scp /tmp/vitoralmeida-tech-test.tar root@IP_DA_VPS:/tmp/
```

Na VPS, a imagem deve ser carregada no storage do usuário `pneuma`, não no
storage de `root`:

```bash
chown pneuma:pneuma /tmp/vitoralmeida-tech-test.tar
sudo -iu pneuma podman load \
  --input /tmp/vitoralmeida-tech-test.tar
sudo -iu pneuma podman images
```

Imagens e containers rootless pertencem ao usuário que os criou e não aparecem
no storage dos demais usuários.

## Podman rootless

O usuário dedicado deve existir, possuir intervalos em `/etc/subuid` e
`/etc/subgid` e ter lingering habilitado:

```bash
id pneuma
grep '^pneuma:' /etc/subuid
grep '^pneuma:' /etc/subgid
loginctl enable-linger pneuma
sudo -iu pneuma podman info --format '{{.Host.Security.Rootless}}'
```

O último comando deve retornar `true`.

O container de staging é iniciado como `pneuma` e expõe a porta somente no
loopback da VPS:

```bash
sudo -iu pneuma podman run --detach \
  --name vitoralmeida-tech-step0 \
  --publish 127.0.0.1:8080:8080 \
  localhost/vitoralmeida.tech:test
```

Para confirmar que não existe uma instância rootful concorrente:

```bash
podman ps
sudo -iu pneuma podman ps
```

O primeiro comando não deve listar o site. O segundo deve listar
`vitoralmeida-tech-step0` em execução.

## Configuração transitória do Caddy

O arquivo principal `/etc/caddy/Caddyfile` importa fragmentos por aplicação:

```caddyfile
import /etc/caddy/applications/*.caddy
```

O fragmento `/etc/caddy/applications/vitoralmeida-tech.caddy` contém:

```caddyfile
:8081 {
    bind 127.0.0.1
    reverse_proxy 127.0.0.1:8080
}
```

Antes de recarregar o serviço:

```bash
caddy validate --config /etc/caddy/Caddyfile
systemctl restart caddy
systemctl is-active caddy
```

O Caddy usa `8081` porque o Nginx de produção já ocupa as portas 80 e 443.

## Configuração transitória do Nginx

O arquivo `/etc/nginx/sites-available/staging.vitoralmeida.tech` contém:

```nginx
server {
    listen 80;
    listen [::]:80;

    server_name staging.vitoralmeida.tech;

    location / {
        proxy_pass http://127.0.0.1:8081;

        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

O site é habilitado e a configuração é verificada antes do reload:

```bash
ln -s \
  /etc/nginx/sites-available/staging.vitoralmeida.tech \
  /etc/nginx/sites-enabled/staging.vitoralmeida.tech
nginx -t
systemctl reload nginx
```

## Validação por fronteira

Na VPS:

```bash
# Container diretamente
curl --fail http://127.0.0.1:8080/healthz

# Caddy para o container
curl --fail http://127.0.0.1:8081/healthz

# Nginx, Caddy e container
curl --fail \
  --header 'Host: staging.vitoralmeida.tech' \
  http://127.0.0.1/healthz
```

Fora da VPS:

```bash
curl --fail http://staging.vitoralmeida.tech/healthz
```

Todos os health checks devem imprimir `ok`.

## HTTPS no staging

HTTPS foi deliberadamente adiado nesta validação. Como não existe um server
block TLS específico para staging, uma requisição a
`https://staging.vitoralmeida.tech` pode cair no virtual host HTTPS de produção
e exibir a página `404.html` do site principal.

Navegadores com HTTPS-first ou HSTS podem trocar `http://` por `https://`
automaticamente. Enquanto TLS não for configurado para staging, o teste externo
canônico é o `curl` HTTP acima.

Antes do ensaio de migração, staging deverá possuir HTTPS válido. Durante esta
topologia transitória, o certificado pertence ao Nginx. Quando o Caddy assumir
as portas públicas, ele deverá administrar o certificado diretamente.

## Reversão

O domínio de produção e seu virtual host não foram alterados. Portanto, uma
falha em staging não exige rollback de produção.

Para interromper somente o runtime de staging:

```bash
sudo -iu pneuma podman stop vitoralmeida-tech-step0
```

Se uma troca de runtime falhar depois que a instância anterior for parada, a
instância anterior deve ser reiniciada antes de investigar:

```bash
sudo -iu pneuma podman start vitoralmeida-tech-step0
```

Não remover a imagem ou o container anterior até que a nova instância tenha
passado pelos health checks interno, intermediário e externo.

## Evidência obtida

- o container executou como Podman rootless sob o usuário `pneuma`;
- `GET /` respondeu HTTP 200;
- `GET /healthz` respondeu HTTP 200 com `ok`;
- as portas 8080 e 8081 ficaram restritas ao loopback;
- o Caddy carregou um fragmento separado por aplicação;
- o hostname público encaminhou tráfego por Nginx, Caddy e container;
- o site de produção permaneceu disponível pelo fluxo anterior.

## Limitações conhecidas

- o container ainda não possui supervisão persistente por Quadlet/systemd;
- HTTPS de staging ainda não foi configurado;
- o Nginx ainda termina o tráfego público;
- a imagem foi transferida manualmente;
- não houve deployment ou rollback executado pelo Pneuma.
