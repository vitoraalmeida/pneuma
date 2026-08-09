# Guia passo a passo — usar o Pneuma numa VPS nova

**Status:** documento vivo — descreve o fluxo de uso atual da v0.1 e deve ser
atualizado na mesma mudança que altera a CLI ou a operação.

Este guia conduz do zero até um site público operado pelo Pneuma numa VPS
nova, passando por bootstrap, instalação, importação, deployment e operação.
O bootstrap detalhado está em
[`operations/vps-bootstrap.md`](operations/vps-bootstrap.md) e pode ser
automatizado por `scripts/bootstrap-vps.sh`; este documento foca no uso
completo, passo a passo.

## 1. Pré-requisitos

- VPS Debian 13 (trixie) com acesso root; Podman >= 4.4 (necessário para o
  gerador Quadlet), Caddy e `curl` instalados.
- Registro DNS A apontando o domínio para o IP da VPS; portas 80 e 443
  liberadas para o Caddy.
- Uma aplicação containerizada com `pneuma.toml` (schema v2) e a seção
  `[delivery]` apontando para o registry (ex.: GHCR).
- O binário `pneuma` instalado em `/usr/local/bin/pneuma`.

## 2. Bootstrap da VPS

Faça o bootstrap do host (pacotes, usuário `pneuma`, subuid/subgid, linger,
diretórios, Caddy e variáveis de ambiente) conforme o guia de bootstrap ou o
script:

```bash
sudo bash scripts/bootstrap-vps.sh \
  git@github.com:USER/pneuma.git \
  git@github.com:USER/vitoralmeida.tech.git
```

Ao final, abra um shell do usuário operador:

```bash
sudo -iu pneuma
```

Confirme o ambiente:

```bash
pneuma version
pneuma doctor
```

## 3. Importar a aplicação

O repositório da aplicação precisa estar clonado em disco (o bootstrap já o
cloneia no workspace). Importe o checkout:

```bash
pneuma app import /var/lib/pneuma/checkouts/vitoralmeida.tech
```

A importação valida o `pneuma.toml`, persiste a especificação e a entrega e é
idempotente — importar de novo não cria duplicidade:

```bash
pneuma app import /var/lib/pneuma/checkouts/vitoralmeida.tech
pneuma app list
```

Anote o nome da aplicação (declarado em `[application] name` no manifesto) —
nos exemplos abaixo, `vitoralmeida-tech-prod`.

## 4. Implantar uma imagem OCI

O caminho oficial recebe uma imagem imutável do CI, pinada por digest:

```bash
pneuma --verbose app deploy vitoralmeida-tech-prod \
  --image ghcr.io/USER/vitoralmeida.tech@sha256:<digest>
```

O repositório da imagem deve coincidir com `[delivery] image`; tags mutáveis
(`:latest`) são rejeitadas. O deployment segue o fluxo: pull → verificação do
digest → Release → runtime candidato → health check interno → ativação →
exposição → verificação pública. O runtime atual só é trocado depois que a
candidata passa no health check.

Caminho alternativo, construindo a imagem localmente a partir de um commit:

```bash
pneuma app deploy-source vitoralmeida-tech-prod \
  /var/lib/pneuma/checkouts/vitoralmeida.tech --revision <commit>
```

## 5. Consultar o estado

```bash
pneuma app status vitoralmeida-tech-prod
pneuma app deployments vitoralmeida-tech-prod
```

`status` reporta o estado desejado e o observado no Podman; `deployments`
mostra o histórico com `DEPLOYMENT | RELEASE | SOURCE | STATUS`.

## 6. Controlar o runtime

```bash
pneuma app stop vitoralmeida-tech-prod
pneuma app start vitoralmeida-tech-prod
```

Os comandos são idempotentes e operam unidades Quadlet do systemd user
manager, de modo que o runtime ativo sobrevive a reboots do host.

## 7. Expor pública ou internamente

```bash
pneuma app visibility set vitoralmeida-tech-prod public
pneuma app visibility set vitoralmeida-tech-prod internal
```

`public` exige domínio válido no manifesto e gera/valida o fragmento do Caddy;
`internal` remove o fragmento sem parar a aplicação. O estado desejado é
persistido separadamente da materialização.

## 8. Rollback

Em caso de problema, retorne ao último deployment saudável:

```bash
pneuma deployment rollback vitoralmeida-tech-prod
```

## 9. Backup e restauração

```bash
pneuma database backup /tmp/pneuma-backup.sqlite3
pneuma database restore /tmp/pneuma-backup.sqlite3
```

O backup é feito de forma consistente; o restore valida o arquivo e faz cópia
do banco atual antes de aplicar.

## 10. Diagnóstico

```bash
pneuma doctor
```

Verifica SQLite e migrações, diretórios, Caddy e sua configuração, Git,
Podman rootless, gerador Quadlet, imagens OCI ativas e espaço em disco.

## 11. Verificação após reboot

Reinicie a VPS e confirme que a unidade Quadlet restaurou o runtime:

```bash
pneuma app status vitoralmeida-tech-prod
curl -I https://vitoralmeida.tech/healthz
```

O `app status` deve observar o runtime como `Running` e o endpoint público
deve responder. Como unidades Quadlet aparecem como `generated` em
`systemctl --user is-enabled`, a verificação de habilitação no boot é o
symlink do gerador:

```bash
ls -l "$XDG_RUNTIME_DIR/systemd/generator/default.target.wants/pneuma-*.service"
```

## Referências

- [`operations/vps-bootstrap.md`](operations/vps-bootstrap.md) — bootstrap
  detalhado do host (Debian 13, Quadlet, GHCR).
- [`operations/public-deployment.md`](operations/public-deployment.md) —
  pré-requisitos e validação do deployment público.
- [`operations/backup-and-restore.md`](operations/backup-and-restore.md) —
  detalhes de backup e recuperação do SQLite.
- [`../README.md`](../README.md) — visão geral, quick start e comandos.
