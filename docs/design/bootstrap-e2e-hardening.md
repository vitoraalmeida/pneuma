# Plano - Bootstrap e E2E Hardening

**Status:** design aprovado. A execução diária e o progresso vivem somente em
`docs/iterations/current-iteration.md`.

**Base:** `8328842` (`docs: record pre-v0.3 consolidation completion`).

## Objetivo

Fortalecer a fundação operacional antes de iniciar `pneuma reconcile`: um host
Debian 13 limpo deve ser provisionado de forma reproduzível e idempotente, a VM
de desenvolvimento deve aplicar os mesmos invariantes de host, e a bateria E2E
deve provar as garantias de deploy, exposição, reboot, backup/restore e acesso
CI no limite real do SSH.

## Escopo

- Tornar `bootstrap-vps.sh` rerun-safe, com parsing explícito, diagnóstico de
  portas correto, `--ref` imutável e configuração Caddy atômica.
- Extrair invariantes comuns de host para uma biblioteca shell concreta usada
  pelo bootstrap de produção e pelo provisionamento da VM.
- Validar usuário `pneuma`, subuid/subgid, linger, diretórios, ambiente,
  rootless Podman e Caddy antes de aceitar um host.
- Criar acceptance test de bootstrap em VM Debian 13 limpa e executar rerun.
- Adicionar lint estático de scripts ao CI com ferramentas version-pinned.
- Tornar obrigatória na VM a exposição HTTPS local, a preservação da release
  ativa diante de candidate falho, rollback real, reboot comprovado, fronteiras
  SSH da chave CI e restore semântico do banco.

## Non-goals

- Não implementar `pneuma reconcile`, watcher de registry, auto-deploy, API,
  TUI, OIDC, RBAC, múltiplos hosts ou novo usuário Linux.
- Não substituir build no host por download de binário de release assinado.
- Não automatizar a VM descartável com cloud image/overlay nesta iteração; o
  fluxo documentado por clone/snapshot libvirt permanece aceitável.
- Não executar E2E destrutivo na VPS de produção; ela permanece destinada a
  smoke não destrutivo de DNS, TLS e reachability reais.

## Decisões fixadas

1. `scripts/bootstrap-vps.sh --ref <ref>` é opção do instalador, não da CLI
   `pneuma`. Ela aceita somente SHA completo de commit ou tag Git; branch e SHA
   abreviado são rejeitados. Todo rerun resolve o ref e força o checkout
   existente para o commit resolvido antes de compilar.
2. A configuração comum de host vive em `scripts/lib/provision-host.sh` como
   funções shell concretas. `bootstrap-vps.sh` controla preflight, fonte,
   build/instalação, chave CI e doctor; `dev-vm/provision-host.sh` usa as mesmas
   invariantes, mas não clona nem compila Pneuma.
3. Um listener nas portas 80/443 é válido no rerun somente se pertencer ao
   Caddy gerenciado e ativo. Nginx, Apache, HTTPD ou qualquer outro listener
   continuam bloqueantes. A inspeção de listener não pode depender de variável
   local fora de escopo.
4. O usuário `pneuma` precisa ter home `/home/pneuma`, shell `/bin/bash`, senha
   bloqueada, ausência de privilégios sudo, subuid/subgid não conflitantes e
   linger habilitado. Estado preexistente incompatível falha explicitamente.
5. `/etc/pneuma/environment` é a fonte canônica de `PNEUMA_*`. O perfil do
   usuário só estabelece o ambiente de sessão rootless e carrega os valores
   necessários sem duplicá-los divergentes.
6. Caddy é alterado por candidate no mesmo filesystem, validação completa,
   backup somente quando o conteúdo muda, instalação atômica e reload. Falha de
   validação não altera o Caddyfile ativo.
7. Scripts administrativos de `scripts/dev-vm/` conectam como root por SSH;
   Podman, systemd user e CLI Pneuma seguem sob `pneuma`. O registry HTTP
   `localhost:5000` é configuração exclusiva da VM de testes.
8. A VM configura `local_certs`, mapeia os domínios fixture para `127.0.0.1` e
   instala a CA local do Caddy na trust store da própria VM. Exposição pública
   HTTPS e a transição para internal são obrigatórias, sem SKIP.
9. ShellCheck e shfmt são instalados em versões fixadas no GitHub Actions. Bats
   só é introduzido para helpers puros extraídos que não possam ser cobertos por
   `bash -n`, ShellCheck e acceptance VM.
10. Não há mudança planejada na CLI ou no código Rust para `--ref`; o contrato
    pertence somente ao bootstrap e aos seus testes.

## Invariantes e evidência

| Área | Invariante | Evidência mínima |
|---|---|---|
| Bootstrap | Debian 13 limpa torna-se host válido | acceptance VM: bootstrap, invariantes e doctor |
| Rerun | Segundo bootstrap preserva estado correto | CI key única, profile/subids sem duplicação, Caddy e doctor verdes |
| Fonte | Instalação é reproduzível | log contém ref solicitado e commit resolvido; checkout detached nesse commit |
| Caddy | Nunca instala candidate inválido | teste de candidate inválido preserva arquivo ativo; rerun aceita Caddy próprio |
| Host | Produção e VM aplicam as mesmas regras | ambos chamam biblioteca comum; testes dos dois caminhos |
| Candidate | Falha não troca versão ativa | v1 permanece Running e responde v1 após deploy falho |
| Rollback | Usa a capability de rollback | `pneuma deployment rollback` restaura v1 após v2 |
| Reboot | Reboot ocorreu e runtime recuperou | boot ID muda, SSH cai/volta, unit/container/status/HTTP corretos |
| Exposure | Public e internal são ortogonais ao runtime | HTTPS público funciona; internal remove rota e preserva container |
| CI SSH | Chave não escapa dispatcher | comandos permitidos passam; comando arbitrário, injection, vazio, PTY e forwardings falham |
| Restore | Restore recupera snapshot lógico | estado pré-backup existe; estado pós-backup não existe |

## Sequência de checkpoints

1. `fix(bootstrap): make preflight rerun-safe and pin source refs`
   - Corrigir diagnóstico de porta, reconhecer Caddy próprio no rerun, endurecer
     argumentos e implementar `--ref` imutável com checkout forçado.
2. `test(bootstrap): make remote assertions reliable`
   - Preservar exit status remoto e validar invariantes do primeiro e segundo
     bootstrap sem mascarar falhas.
3. `refactor(host): share provisioning invariants`
   - Extrair `scripts/lib/provision-host.sh` e fazer bootstrap VPS e VM usarem
     as mesmas funções de pacote, usuário, IDs subordinados, diretórios,
     ambiente, Caddy, linger e rootless Podman.
4. `fix(host): enforce account and subordinate-id invariants`
   - Rejeitar usuário inseguro e ranges incompatíveis; tornar ownership e modos
     verificáveis e idempotentes.
5. `refactor(bootstrap): make Caddy configuration atomic`
   - Validar candidate, instalar atomicamente, criar backup sensível a conteúdo
     e preservar configuração anterior em falha.
6. `test(bootstrap): add clean-host rerun acceptance`
   - Executar bootstrap em Debian 13 limpa com `--ref`, validar invariantes,
     rerodar e revalidar; destruir clone descartável após a evidência.
7. `ci: lint shell scripts`
   - Adicionar `bash -n`, ShellCheck e shfmt version-pinned a todos os scripts.
8. `test(e2e): preserve active release and use real rollback`
   - Provar candidate falho preservando v1 e rollback real v2 para v1.
9. `test(e2e): prove reboot recovery`
   - Provar indisponibilidade, alteração de boot ID e recuperação de unit,
     container, status e HTTP.
10. `test(e2e): require local HTTPS exposure and CI boundaries`
    - Configurar local_certs/CA/hosts, provar public/internal e os casos SSH
      permitidos e negados, inclusive PTY e forwardings.
11. `test(e2e): verify restore semantics and document operations`
    - Provar restore semântico, sincronizar documentação operacional e executar
      a regressão final em VM e os gates finais.

## Critério de conclusão

- Todos os checkpoints passam os quatro gates Rust e a validação shell
  proporcional; CI executa lint shell version-pinned.
- A acceptance VM limpa prova primeiro bootstrap e rerun com ref imutável.
- A regressão E2E VM não possui skips para HTTPS público, rollback, reboot, CI
  SSH ou backup/restore; qualquer skip restante é explícito e aceito antes do
  encerramento.
- A documentação distingue bootstrap descartável, VM E2E persistente e smoke de
  produção, e descreve o contrato de privilégios e de `--ref`.
- A iteração só fecha após os quatro gates no commit final e as regressões VM
  exigidas passarem.
