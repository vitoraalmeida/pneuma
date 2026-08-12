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
- [ ] Corrigir assertions remotas do acceptance test de bootstrap.
- [ ] Compartilhar invariantes de provisionamento entre VPS e VM.
- [ ] Impor invariantes de conta e IDs subordinados.
- [ ] Tornar configuração Caddy atômica e idempotente.
- [ ] Provar bootstrap e rerun em host Debian 13 limpo.
- [ ] Adicionar lint shell version-pinned ao CI.
- [ ] Provar candidate falho preservando release ativa e rollback real.
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
