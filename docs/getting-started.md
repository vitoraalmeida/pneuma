# Configuração completa de um host Pneuma

Guia passo a passo para tornar uma VPS Debian 13 num host Pneuma de produção e
conectar um repositório de aplicação a ele via GitHub Actions. Cobre: geração
das chaves, execução do `bootstrap-vps.sh`, import e deploy, e configuração do
workflow de deploy.

O `scripts/bootstrap-vps.sh` instala tudo (pacotes, usuário `pneuma`, Podman
rootless, Caddy, binário) **e** prepara a identidade para o CI. Depois que ele
termina com sucesso, o host está pronto para importar e deployar aplicações.

## 1. Pré-requisitos

- VPS Debian 13 (trixie) com acesso root por SSH. Debian 12 **não** serve
  (Podman 4.3.1 sem o gerador Quadlet necessário ao Pneuma).
- Acesso à internet, DNS resolvendo, portas 80/443 livres e pelo menos 2 GiB de
  RAM e 3 GiB de disco livre (o script confere tudo antes de mexer no sistema e
  aborta com mensagens acionáveis se algo faltar).
- O domínio público das aplicações (e o `*.staging`, se usar pré-produção)
  apontando (DNS A/AAAA) para o IP da VPS.
- Ignorar nginx/apache rodando na VPS — são conflito bloqueante.

## 2. Gerar as chaves

O host Pneuma usa duas identidades diferentes. São funções distintas, não as
confunda:

### 2.1. Chave do usuário `pneuma` (acesso do host a repositórios Git)

Usada pelo próprio Pneuma para `git clone`/`git fetch` no deploy por branch — só
necessária se o repositório **da aplicação** (ou do próprio Pneuma) for
privado via SSH. **O bootstrap gera essa chave automaticamente** quando você
passa uma URL de fonte SSH:

```bash
bash scripts/bootstrap-vps.sh git@github.com:USER/pneuma.git
```

Na primeira execução o script cria `~pneuma/.ssh/id_ed25519`, imprime a chave
pública e para. Você adiciona essa pública como *deploy key* read-only do(s)
repositório(s) privado(s) e reexecuta o script para continuar.

> Se a URL de fonte do Pneuma for pública via HTTPS (URL comum ao clonar o
> próprio Pneuma, que não é privado), essa chave nunca é criada. Nesse caso,
> para aplicações com repositórios Git **privados**, gere uma chave para o
> usuário `pneuma` manualmente na VPS ou configure o acesso de outra forma; o
> bootstrap serve apenas o fluxo HTTPS-público/SSH-privado da fonte do Pneuma.

### 2.2. Chave do CI (deploy via GitHub Actions)

Usada nas máquinas do GitHub Actions para autenticar no host como `pneuma` e
disparar deploys. Como o próprio bootstrap ensina: **gere o par numa máquina
confiável, nunca na VPS** — a privada vira secret do GitHub, e só a pública é
entregue ao script:

```bash
ssh-keygen -t ed25519 -f ~/.ssh/pneuma-ci -N "" -C "pneuma-ci deploy key"
```

Isso cria `~/.ssh/pneuma-ci` (privada) e `~/.ssh/pneuma-ci.pub` (pública). O
script aceita `ssh-ed25519`, `ssh-rsa` e `ecdsa-sha2-nistp*`.

## 3. Executar o bootstrap

Todas as chaves em mãos, rode como root na VPS, passando a **pública** do CI
com `--ci-public-key <caminho>`:

```bash
bash bootstrap-vps.sh \
  git@github.com:USER/pneuma.git \
  --ci-public-key ~/.ssh/pneuma-ci.pub
```

O script instala a chave pública do CI em `~pneuma/.ssh/authorized_keys` com
`restrict,command="/usr/local/bin/pneuma ci dispatch"` — ou seja, quem autentica
com essa chave **só** pode executar o dispatcher restrito (nada de shell). Ao
final, além de `pneuma doctor`, ele imprime o mesmo `DEPLOY_SSH_KEY` de aviso e
o comando de teste.

Existem duas formas de levar o arquivo até a VPS:

```bash
# A) copiar com scp e ler de lá
scp ~/.ssh/pneuma-ci.pub root@<ip>:pneuma-ci.pub
ssh root@<ip> 'bash scripts/bootstrap-vps.sh git@github.com:USER/pneuma.git --ci-public-key pneuma-ci.pub'
# B) colar o conteúdo numa chave direta (sem arquivo temporário)
ssh root@<ip> 'bash bootstrap-vps.sh git@github.com:USER/pneuma.git --ci-public-key /dev/stdin' < ~/.ssh/pneuma-ci.pub
```

As invariantes de host aplicadas pelo bootstrap (pacotes de runtime, usuário e
grupo `pneuma`, subids, linger, diretórios, ambiente canônico, Caddy e Podman
rootless) vivem numa biblioteca comum, `scripts/lib/provision-host.sh`,
compartilhada com o provisionamento da VM de desenvolvimento. A biblioteca não
muda o comportamento exclusivo do bootstrap: clonar a fonte, compilar e
instalar o binário e instalar a chave CI continuam no `bootstrap-vps.sh`.

### 3.1. Fixar a versão do Pneuma com `--ref`

Para instalações reproduzíveis, o bootstrap aceita `--ref` com **apenas** SHA
completo de commit (`[0-9a-f]{40}`) ou tag Git existente; branch e SHA abreviado
são rejeitados antes de qualquer mudança no host:

```bash
bash bootstrap-vps.sh \
  git@github.com:USER/pneuma.git \
  --ci-public-key ~/.ssh/pneuma-ci.pub \
  --ref v0.3.0
```

Cada execução (inclusive reruns) resolve o `--ref`, faz checkout detached
**forçado** do commit resolvido e compila exatamente esse commit;
configurações de APT, usuário, subids e Caddy permanecem idempotentes. No rerun,
o próprio Caddy gerenciado e ativo pode ocupar as portas 80/443; qualquer outro
processo nessas portas continua bloqueante. Sem `--ref`, o script compila o
branch default do repositório, como antes.

### 3.2. Confirmação

Como `pneuma` (login direto com a chave de provisionamento ou `sudo -iu
pneuma`):

```bash
pneuma doctor        # todos os checks de host aprovados
pneuma app list      # vazio (nenhuma aplicação ainda)
```

Da sua máquina, confirme a identidade do CI autenticando com a **privada**:
deveria responder a `version`:

```bash
ssh -i ~/.ssh/pneuma-ci pneuma@<ip-do-host> "version"
```

Se respondeu, o host está pronto para ser gerenciado pelo GitHub Actions.

## 4. Importar e deployar manualmente

O fluxo padrão do Pneuma com CI é: CI constrói e publica a imagem (tag com o
SHA do commit) e, em seguida, pede o deploy ao host. Em qualquer momento dá
para importar e deployar manualmente a partir do host.

### 4.1. Importar a aplicação

Entre como `pneuma` e importe do repositório Git, apontando o manifest da
entrega com `--manifest`:

```bash
sudo -iu pneuma
pneuma app import https://github.com/owner/my-app --manifest deploy/staging/pneuma.toml
```

O Pneuma clona o repositório **somente temporariamente** (lê o `pneuma.toml`,
persiste a aplicação e remove o checkout) e registra a aplicação com a entrega
declarada (imagem OCI, porta, healthcheck, visibilidade). O `--manifest` é o
caminho do `pneuma.toml` **dentro do repositório**. `app import` aceita apenas
URLs Git; paths locais são rejeitados e `file://` é reservado a repositórios de
teste locais.

### 4.2. Deploy por branch (recomendado com CI)

O CI já publicou a imagem com a tag do SHA; o Pneuma resolve o branch → SHA →
tag → digest:

```bash
pneuma app deploy my-app --branch staging
```

### 4.3. Deploy por digest (imutável, manual)

```bash
pneuma app deploy my-app --image ghcr.io/owner/my-app@sha256:<digest>
```

O deploy valida a imagem (pull + health check) antes de promover; se falhar, a
versão anterior continua ativa. Acompanhe com:

```bash
pneuma app status my-app
pneuma app deployments my-app
```

## 5. Configurar o GitHub Actions

O workflow do repositório da aplicação precisa de secrets e variables de
repositório ou de conta. Secrets são obrigatoriamente `DEPLOY_SSH_KEY` e
`DEPLOY_KNOWN_HOSTS`; as variables são `DEPLOY_HOST` e `DEPLOY_USER`.

### 5.1. Secrets

No GitHub (perfil → Settings → Secrets and variables → Actions → New
repository secret), crie:

- **`DEPLOY_SSH_KEY`** — conteúdo da chave privada do CI, o arquivo
  `~/.ssh/pneuma-ci`. Cole o texto de arquivo inteiro, incluindo o bloco
  `-----BEGIN OPENSSH PRIVATE KEY-----`/`-----END...-----` e com a última linha
  terminada em quebra de linha. Erros aqui são a causa #1 de
  `Permission denied (publickey)`.
- **`DEPLOY_KNOWN_HOSTS`** — linha(s) de fingerprint do host, obtidas por
  `ssh-keyscan`. Com a VPS já acessível:

```bash
ssh-keyscan 46.202.150.155   # ou o hostname/domínio do host
```

Cole a saída como valor (se você também acessa o IP diretamente, inclua as
linhas do IP e do hostname). Não adicione artefato além das linhas
`<host> <algoritmo> <chave>`.

> Dica: com impedimento de acesso no momento da configuração, você pode gerar o
> known_hosts localmente com `ssh-keyscan` pela mesma rede; o bom fingerprint
> `ecdsa-sha2-*`/`ssh-ed25519` apresentado ao primeiro `ssh` continua o mesmo.

### 5.2. Variables

No mesmo painel, em "Variables", crie:

- **`DEPLOY_HOST`** — IP ou hostname da VPS (ex.: `46.202.150.155`).
- **`DEPLOY_USER`** — `pneuma` (não admin).

Manter IP/host como variable permite trocar de VPS sem editar o workflow.

### 5.3. (Opcional) scope de conta

Como a chave é restrita ao dispatcher, um único par de secrets a nível de conta
serve todos os repositórios da conta: acesse **Settings da conta → Secrets and
variables → Actions** e crie os mesmos quatro itens uma vez. Repositórios que
precisem de hosts diferentes criam secrets próprios sobrescrevendo.

### 5.4. Workflow de exemplo

O repositório da aplicação precisa de um workflow que (1) construa e publique a
imagem OCI e (2) dispare o deploy. Depois de publicada a imagem com tag do SHA
e do branch, o passo de deploy replica:

```yaml
- name: Deploy to staging
  env:
    DEPLOY_SSH_KEY: ${{ secrets.DEPLOY_SSH_KEY }}
    DEPLOY_KNOWN_HOSTS: ${{ secrets.DEPLOY_KNOWN_HOSTS }}
  run: |
    mkdir -p ~/.ssh
    printf '%s\n' "$DEPLOY_SSH_KEY" > ~/.ssh/deploy_key
    printf '%s\n' "$DEPLOY_KNOWN_HOSTS" > ~/.ssh/known_hosts
    chmod 600 ~/.ssh/deploy_key
    ssh -i ~/.ssh/deploy_key -o BatchMode=yes \
      ${{ vars.DEPLOY_USER }}@${{ vars.DEPLOY_HOST }} \
      "deploy my-app staging"
```

- O comando SSH é SIM o dispatcher: `deploy <application> <branch>` (o binário
  `pneuma ci dispatch` é invocado via forced command; **não** escreva
  `pneuma app deploy ... --branch ...` — seria rejeitado).
- `my-app` precisa já estar importado no host (seção 4.1) **antes** do primeiro
  deploy via CI.
- O workflow completo (build da imagem, smoke test, push para GHCR, deploy por
  staging/main) pode ser copiado do repositório
  `github.com/vitoraalmeida/vitoralmeida.tech`, arquivo
  `.github/workflows/deploy.yml`.

## 6. Checagem final

```bash
# No host
sudo -iu pneuma
pneuma doctor          # ok

# Na sua máquina
ssh -i ~/.ssh/pneuma-ci pneuma@<host> "version"   # responde com a versão

# Após um push no branch do workflow
# → workflow publica a imagem e pede deploy
# Na VPS: pneuma status e pneuma list devem refletir o novo release Running
```

O ciclo fecha: push no Git → CI constrói/publica → `deploy <app> <branch>` →
Pneuma resolve, valida e promove; Caddy expõe a aplicação pública automaticamente.