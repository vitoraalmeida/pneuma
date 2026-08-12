# Iteração atual

**Status:** em andamento

**Base:** `8328842` (`docs: record pre-v0.3 consolidation completion`)

**Design aprovado:** [`design/bootstrap-e2e-hardening.md`](../design/bootstrap-e2e-hardening.md)

## Iteração - Hardening de bootstrap, VM e E2E

Objetivo: tornar o bootstrap e a regressão em VM reproduzíveis, idempotentes e
capazes de provar as garantias operacionais necessárias antes de reconciliation.

### Escopo e non-goals

- Bootstrap VPS e provisionamento VM passam a compartilhar invariantes de host;
  acceptance VM limpa e E2E tornam-se critérios obrigatórios.
- A iteração não implementa `pneuma reconcile`, watcher de registry, auto-deploy,
  API, TUI, OIDC, RBAC, múltiplos hosts, novo usuário Linux ou download de
  binário pré-compilado.
- A execução segue os checkpoints do design na ordem abaixo. O primeiro item
  desmarcado é o próximo trabalho autorizado.

## Checkpoints

- [x] Abrir a iteração e registrar o design aprovado.
- [x] Tornar preflight rerun-safe e fixar fonte com `--ref` imutável.
    Resultado: parsing explícito de `--ci-public-key`/`--ref` rejeita valores
    ausentes, opções desconhecidas, branch, SHA abreviado e refs inválidas;
    `--ref` aceita tag ou SHA completo e força checkout detached do commit
    resolvido em todo rerun; listener de 80/443 aceita Caddy ativo no rerun e
    bloqueia qualquer outro dono. Gates verdes e lint shell sem warnings.
- [x] Corrigir assertions remotas do acceptance test de bootstrap.
    Resultado: `remote_assert`/`remote_assert_rejected`/`ci_assert_*` capturam
    stdout/stderr em logs, preservam o exit status do `ssh` e exigem sucesso
    remoto para conteúdo; acceptance limpo Debian 13 por SHA `2bdc512` passou
    com 82 PASS/0 FAIL, e assertion remota forçada produziu 1 FAIL e saída não
    zero; fixture/deploy funcional segue em `dev-vm/test-all.sh`.
- [x] Compartilhar invariantes de provisionamento entre VPS e VM.
    Resultado: `scripts/lib/provision-host.sh` centraliza as invariantes de
    runtime; checks estáticos/ShellCheck e quatro gates verdes, provisionamento
    VM descartável validado e acceptance bootstrap por SHA com 82 PASS/0 FAIL.
- [x] Impor invariantes de conta e IDs subordinados.
    Resultado: preflight somente leitura rejeita conta insegura e ranges
    malformados, duplicados, sobrepostos ou insuficientes antes de mutações;
    subids/subgids alternativos seguros são preservados e ausência seleciona o
    primeiro range livre de 65.536 IDs a partir de 100000, sem mudar outro
    usuário; linger é confirmado e diretórios recuperam modos no rerun. Fixture
    9 PASS/0 FAIL, ShellCheck e quatro gates Rust verdes; clone Debian 13
    descartável validou o fallback após range `dev` conflitante e idempotência.
- [x] Tornar configuração Caddy atômica e idempotente.
    Resultado: candidate no filesystem de destino é validado antes da troca por
    rename, rerun idêntico não cria backup nem recarrega Caddy, e falha preserva
    o arquivo ativo. Preflight aceita listener Caddy apenas com Caddyfile válido;
    ShellCheck e quatro gates Rust verdes, clone Debian 13 descartável validou
    primeira instalação, rerun, backup por mudança e candidate inválido.
- [x] Provar bootstrap e rerun em host Debian 13 limpo.
    Resultado: clone nova de `pneuma-dev-base` executou bootstrap e dois reruns
    pelo SHA imutável `11b10111f59a6fea09524fc4bd78f1109e830cd3`, com 87 PASS/0
    FAIL. Validou range livre após `dev:100000:65536`, conta, linger,
    diretórios, ambiente, Caddy, Podman rootless, binário, chave CI e doctor;
    clone descartável foi destruída.
- [x] Adicionar lint shell version-pinned ao CI.
    Resultado: job `shell` instala ShellCheck 0.10.0 e shfmt 3.10.0 fixados e
    executa `bash -n`, ShellCheck e shfmt em todos os scripts rastreados; scripts
    foram formatados e variáveis não usadas removidas. Checks shell e quatro
    gates Rust verdes.
- [x] Provar candidate falho preservando release ativa e rollback real.
    Resultado: E2E publica candidate unhealthy no repositório permitido de
    `healthy-http`, exige deployment `Deploy/Failed`, runtime Running e body v1;
    depois promove v2 e chama `pneuma deployment rollback healthy-http`, exigindo
    body v1 e histórico `Rollback/Succeeded`. Clone Debian 13 descartável passou
    o ciclo completo e foi destruída.
- [ ] Provar reboot e recuperação por boot ID.
- [ ] Tornar HTTPS local e fronteiras SSH CI obrigatórios no E2E.
- [ ] Provar restore semântico, sincronizar docs e executar regressão final.

## Critérios de aceite

- [ ] Bootstrap aceita somente `--ref` SHA completo ou tag e reinstala o commit
  resolvido em todo rerun.
- [ ] Host limpo e rerun validam invariantes de usuário, subids, linger,
  diretórios, ambiente, Caddy, rootless Podman, binário e chave CI.
- [ ] VPS e VM chamam uma implementação comum das invariantes de host.
- [ ] Caddy é atualizado atomicamente e o preflight aceita apenas Caddy próprio
  nas portas 80/443 durante rerun.
- [ ] CI executa `bash -n`, ShellCheck e shfmt com versões fixadas.
- [ ] E2E prova candidate falho preservando v1, rollback real, reboot real,
  HTTPS público/internal, fronteiras da chave CI e restore semântico.
- [ ] Quatro gates e regressões de bootstrap/E2E VM exigidas estão verdes, sem
  skips não aceitos.

## Bloqueadores

Nenhum.

## Validação final

Pendente: gates e regressões proporcionais a cada checkpoint; repetição integral
no commit final da iteração.
