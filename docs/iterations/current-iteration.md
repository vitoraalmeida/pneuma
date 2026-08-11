# Iteração atual

**Status:** concluída

**Atualizado em:** 10 de agosto de 2026

## Iteração — v0.2 Git-aware OCI Delivery

Objetivo: tornar o Pneuma um sistema de **discovery e deployment** de artifacts
OCI produzidos pelo CI. Pneuma deixa de construir aplicações; o fluxo passa a
ser `Git branch → commit → OCI digest → Release → deployment`.

### Critérios de aceite (Definition of Done)

- `pneuma app import <git-url> --manifest deploy/staging/pneuma.toml` registra
  a aplicação a partir de um repositório Git remoto, sem deployment;
- `pneuma app deploy vitoralmeida-tech-staging --branch staging` encontra e
  implanta o artifact do commit da branch, sem clone manual, build local,
  descoberta manual de digest ou edição manual de Caddy;
- `pneuma app deploy <app> --branch <branch>` e `--image <repo>@sha256:<hex>`
  são mutuamente exclusivos;
- uso de cases state-changing relevantes não contêm SQL diretamente;
  persistência e atomicidade ficam nos SQLite stores.

### Fases

- [x] **A — simplificar:** remover `deploy-source`, `deployment_deploy_source`,
  `local_build`, `[build]`, `application_build_specs`, import por path local,
  source local e checkout permanente de build.
- [x] **B — separar persistência:** criar `SqliteApplicationStore`,
  `SqliteDeploymentStore`, `SqliteRuntimeStore`, `SqliteReleaseStore`; migrar
  create/transition/fail/promotion, runtime persistence e release/rollback.
- [x] **C — novo schema:** manifest v3 (sem `[source]`/`[build]`),
  `deploy/<environment>/pneuma.toml`, novas migrations (nunca alterar as
  históricas).
- [x] **D — import Git remoto:** `app import <git-url>`, `--manifest`, checkout
  temporário, persistir `repository_url`/`manifest_path`, idempotência.
- [x] **E — Git resolution:** adapter de Git remoto, `resolve_branch()`,
  `CommitSha`, erros de auth/repositório/branch.
- [x] **F — OCI discovery:** convenção `image:<commit>`, resolver tag do commit
  → digest, nunca devolver tag mutável ao engine.
- [x] **G — deploy por branch:** `DeployByBranch`
  (`deployment_deploy_branch.rs`), `--branch`, exclusão mútua com `--image`,
  persistir `source_revision`.
- [x] **H — aplicação real:** mover manifestos do website, importar staging,
  testar `--branch staging`, automatizar staging no Actions, importar
  production, testar `--branch main` e rollback.

## Refatoração pós-v0.2 (10 de agosto de 2026)

Após a conclusão da v0.2, o `deployment_deploy_release.rs` (1259 linhas) foi
refatorado em módulos coesos para melhorar a manutenibilidade:

- `deployment_progress.rs` — reporting de progresso
- `deployment_runtime_cleanup.rs` — cleanup de candidates e runtimes antigos
- `deployment_start_candidate.rs` — criação do runtime candidato
- `deployment_activate_public.rs` — ativação pública (health + Caddy)

O orquestrador principal (`deployment_deploy_release.rs`) reduziu de 1259 para
676 linhas, expressando claramente o algoritmo de deployment no nível de
aplicação. A refatoração incluiu testes de caracterização, a correção de uma
regressão no cleanup de candidatas que falham o health check e a validação na
VM local (`scripts/dev-vm/e2e.sh`).
