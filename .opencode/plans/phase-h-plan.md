# Plano da Fase H — Aplicação Real

**Status:** pendente de execução

**Atualizado em:** 10 de agosto de 2026

## Objetivo

Migrar o website `vitoralmeida.tech` para o fluxo Pneuma v0.2, com deploy automatizado para staging (push) e manual para production (merge para main).

## Estado atual

- **Repositório:** `/home/xpectre/Projects/vitoralmeida.tech`
- **Branches:** `main` (production) e `staging` (pré-production)
- **Manifesto atual:** `pneuma.toml` na raiz, schema v2 (obsoleto)
- **GitHub Actions:** `deploy.yml` builda e pusha imagens para GHCR com tags `<commit-sha>` e `<branch>`
- **Deploy atual:** manual (instruções impressas no workflow)

## Decisões tomadas

1. ✅ **Credenciais SSH:** reutilizar `DEPLOY_SSH_KEY` e `DEPLOY_KNOWN_HOSTS` já configurados no GitHub
2. ✅ **deploy-static.yml:** manter como fallback (não remover)
3. ✅ **Estratégia de branches:**
   - `staging` → deploy automático
   - `main` → deploy manual (com environment protection)
4. ✅ **Domínios:**
   - Staging: `staging.vitoralmeida.tech` (público, já existe)
   - Production: `vitoralmeida.tech` (público)
5. ✅ **Testes:** testar manualmente na VM antes de automatizar

## Plano de execução

### Etapa 1: Criar estrutura de manifests v3

**Arquivos a criar:**

1. `deploy/staging/pneuma.toml`
   ```toml
   schema_version = 3
   
   [system]
   name = "personal-website"
   
   [application]
   name = "vitoralmeida-tech-staging"
   
   [delivery]
   type = "oci"
   image = "ghcr.io/vitoraalmeida/vitoralmeida.tech"
   
   [runtime]
   container_port = 8080
   healthcheck_path = "/healthz"
   expected_status = 200
   
   [exposure]
   default_visibility = "public"
   domain = "staging.vitoralmeida.tech"
   ```

2. `deploy/production/pneuma.toml`
   ```toml
   schema_version = 3
   
   [system]
   name = "personal-website"
   
   [application]
   name = "vitoralmeida-tech-production"
   
   [delivery]
   type = "oci"
   image = "ghcr.io/vitoraalmeida/vitoralmeida.tech"
   
   [runtime]
   container_port = 8080
   healthcheck_path = "/healthz"
   expected_status = 200
   
   [exposure]
   default_visibility = "public"
   domain = "vitoralmeida.tech"
   ```

3. **Remover:** `pneuma.toml` da raiz (substituído pelos manifests em `deploy/`)

### Etapa 2: Testes manuais na VM (pré-automação)

**Pré-requisito:** VM precisa estar ligada (`virsh start pneuma-dev`)

**Sequência de testes:**

1. **Importar staging:**
   ```bash
   ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && \
     pneuma app import https://github.com/vitoraalmeida/vitoralmeida.tech \
     --manifest deploy/staging/pneuma.toml"'
   ```

2. **Importar production:**
   ```bash
   ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && \
     pneuma app import https://github.com/vitoraalmeida/vitoralmeida.tech \
     --manifest deploy/production/pneuma.toml"'
   ```

3. **Verificar imports:**
   ```bash
   ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app list"'
   ```

4. **Deploy staging (branch staging):**
   - Fazer push para branch `staging` no GitHub
   - Aguardar CI buildar e pushar imagem para GHCR
   - Executar:
     ```bash
     ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && \
       pneuma app deploy vitoralmeida-tech-staging --branch staging"'
     ```
   - Verificar: `pneuma app status vitoralmeida-tech-staging`
   - Validar: `curl https://staging.vitoralmeida.tech/`

5. **Deploy production (branch main):**
   - Fazer merge de `staging` para `main`
   - Aguardar CI buildar e pushar imagem
   - Executar:
     ```bash
     ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && \
       pneuma app deploy vitoralmeida-tech-production --branch main"'
     ```
   - Verificar: `pneuma app status vitoralmeida-tech-production`
   - Validar: `curl https://vitoralmeida.tech/`

6. **Testar rollback:**
   ```bash
   ssh pneuma-dev 'runuser -u pneuma -- bash -lc "cd \$HOME && \
     pneuma deployment rollback vitoralmeida-tech-production"'
   ```
   - Verificar que voltou para a versão anterior

### Etapa 3: Automatizar staging no GitHub Actions

**Arquivo a modificar:** `.github/workflows/deploy.yml`

**Adicionar após o step "Push image to GHCR":**

```yaml
- name: Deploy to staging
  if: github.ref_name == 'staging'
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
      "runuser -u pneuma -- bash -lc 'cd \$HOME && \
        pneuma app deploy vitoralmeida-tech-staging --branch staging'"
```

### Etapa 4: Automatizar production manual

**Arquivo a modificar:** `.github/workflows/deploy.yml`

**Adicionar após o step de staging:**

```yaml
- name: Deploy to production
  if: github.ref_name == 'main'
  environment: production
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
      "runuser -u pneuma -- bash -lc 'cd \$HOME && \
        pneuma app deploy vitoralmeida-tech-production --branch main'"
```

**Nota:** O `environment: production` exige aprovação manual no GitHub (precisa configurar).

### Etapa 5: Configurar environment protection no GitHub

**Passos:**
1. Ir em Settings → Environments → New environment
2. Criar environment `production`
3. Habilitar "Required reviewers" e adicionar revisores
4. Salvar

### Etapa 6: Documentação

**Arquivo a criar/atualizar:**
1. `docs/deployment.md` — documentar o fluxo de deploy (staging automático, production manual)
2. `README.md` — atualizar seção de deployment com o novo fluxo

### Etapa 7: Commit e push

**Sequência de commits:**

1. `feat: migrate to schema v3 with staging/production manifests`
   - Criar `deploy/staging/pneuma.toml` e `deploy/production/pneuma.toml`
   - Remover `pneuma.toml` da raiz
   
2. `feat: automate staging and production deploys`
   - Modificar `.github/workflows/deploy.yml`
   
3. `docs: document deployment workflow`
   - Criar/atualizar documentação

**Push para staging:**
```bash
git checkout staging
git merge main  # ou criar branch feature
git push origin staging
# CI builda → push imagem → deploy automático staging
```

**Push para production:**
```bash
git checkout main
git merge staging
git push origin main
# CI builda → push imagem → deploy manual production (após aprovação)
```

## Riscos e considerações

1. **DNS:** `staging.vitoralmeida.tech` já existe ✅
2. **Certificados TLS:** Caddy emite automaticamente para domínios públicos
3. **Secrets no GitHub:** `DEPLOY_SSH_KEY` e `DEPLOY_KNOWN_HOSTS` já existem ✅
4. **Variáveis no GitHub:** `DEPLOY_USER`, `DEPLOY_HOST`, `DEPLOY_PORT` já existem ✅
5. **Environment protection:** precisa configurar `production` no GitHub
6. **Rollback:** testar rollback em staging antes de production

## Ordem de execução

1. [ ] Criar manifests v3 (staging/production)
2. [ ] Ligar VM e testar imports manualmente
3. [ ] Testar deploy staging (push para branch `staging`)
4. [ ] Testar deploy production (merge para `main`)
5. [ ] Testar rollback
6. [ ] Configurar environment protection no GitHub
7. [ ] Automatizar staging no GitHub Actions
8. [ ] Automatizar production manual
9. [ ] Documentar fluxo
10. [ ] Commit e push para staging
11. [ ] Validar deploy automático staging
12. [ ] Merge para main e validar deploy production

## Próximos passos ao retomar

1. Ligar VM: `virsh start pneuma-dev`
2. Verificar IP da VM: `virsh -c qemu:///system domifaddr pneuma-dev`
3. Atualizar `~/.ssh/config` se necessário
4. Executar Etapa 1 (criar manifests)
5. Executar Etapa 2 (testes manuais)
6. Continuar com as demais etapas
