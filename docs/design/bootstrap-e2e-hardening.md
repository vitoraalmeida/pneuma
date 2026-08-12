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

## Plano detalhado de execução

Cada checkpoint abaixo gera um commit convencional próprio, atualiza somente o
item correspondente no tracker e executa os quatro gates Rust. Uma falha em VM
ou ambiente deixa o item aberto com comando, diagnóstico, último checkpoint
verde e próxima ação segura.

### 1. Preflight rerun-safe e `--ref`

**Arquivos previstos:** `scripts/bootstrap-vps.sh`,
`scripts/test-bootstrap-vps.sh`, `docs/getting-started.md`,
`docs/iterations/current-iteration.md`.

**Mudanças:**

- Substituir o parsing atual por validação explícita de `--ci-public-key` e
  `--ref`, rejeitando valores ausentes, opções desconhecidas, branch e SHA
  abreviado antes de alterar o host.
- Aceitar uma tag existente ou `[0-9a-f]{40}`. Após clone/fetch, resolver a tag
  para commit, verificar que o SHA completo resolve para commit e fazer checkout
  detached forçado desse commit a cada rerun. Sem `--ref`, manter o branch
  default remoto como comportamento atual.
- Imprimir URL, ref solicitado quando existir e SHA resolvido antes do build.
- Encapsular a leitura de listeners de 80/443 numa função que não exponha
  variáveis locais. Se a porta estiver ocupada, permitir somente Caddy ativo;
  manter erro acionável para outro proprietário e para nginx/apache/httpd.
- Validar pelo menos: sem URL, opção desconhecida, `--ci-public-key` sem valor,
  arquivo de chave inexistente, `--ref` sem valor, branch, SHA abreviado, SHA
  inválido, tag ausente, tag válida e SHA válido.

**Verificação focal:** `bash -n` e ShellCheck disponível localmente; teste em VM
deve provar primeiro run por tag/SHA e rerun que reinstala o mesmo commit. Não
alterar ainda o contrato de usuário, Caddy atômico ou a biblioteca comum.

### 2. Assertions remotas confiáveis

**Arquivos previstos:** `scripts/test-bootstrap-vps.sh`,
`docs/iterations/current-iteration.md`.

**Mudanças:**

- Criar helpers concretos que capturem stdout/stderr em arquivos de log e
  preservem o exit status do `ssh`, sem `|| true` nas assertions.
- Fazer cada assertion de conteúdo exigir também sucesso remoto, salvo testes
  que afirmam rejeição.
- Separar bootstrap acceptance de fixture/deploy funcional: este script valida
  somente host limpo, bootstrap, rerun e dispatcher básico; o E2E funcional
  continua em `scripts/dev-vm/test-all.sh`.
- Adicionar assertions explícitas de UID/home/shell, senha bloqueada, ausência
  de grupo sudo, `/etc/subuid`, `/etc/subgid`, linger, donos/modos de diretórios,
  ambiente canônico, binário root-owned, Caddy ativo/válido, rootless Podman,
  Quadlet, CI key e forced command.
- Após rerun, verificar uma única chave CI, uma ocorrência por linha gerenciada
  no profile, entradas subuid/subgid estáveis e doctor verde.

**Verificação focal:** executar contra clone Debian 13 limpo. Forçar uma
assertion remota sabidamente falsa deve incrementar FAIL e retornar não zero;
não deve ser possível reportar PASS com SSH falho.

### 3. Biblioteca comum de provisionamento

**Arquivos previstos:** `scripts/lib/provision-host.sh` novo,
`scripts/bootstrap-vps.sh`, `scripts/dev-vm/provision-host.sh`, scripts de teste
diretamente afetados, `docs/operations/dev-vm-tutorial.md`,
`docs/getting-started.md`, `docs/iterations/current-iteration.md`.

**Mudanças:**

- Mover para funções concretas, sem framework shell: instalação de pacotes,
  descoberta do generator Quadlet, usuário/grupo, subuid/subgid, linger,
  diretórios, ambiente, Caddy baseline, início do user manager e verificação
  rootless.
- A biblioteca não faz clone, checkout, Rustup, cargo build, instalação do
  binário, CI key nem `pneuma doctor`; essas diferenças ficam no chamador.
- Ambos os chamadores devem sourcear a mesma biblioteca com caminho calculado a
  partir do próprio script, para não depender do diretório atual.
- Preservar execução como root e manter comandos de runtime sob `pneuma`.

**Verificação focal:** `bash -n` nos três arquivos; VM de bootstrap e VM de
desenvolvimento confirmam o mesmo conjunto de invariantes. Não introduzir trait,
daemon ou alteração Rust.

### 4. Invariantes de conta e IDs subordinados

**Arquivos previstos:** `scripts/lib/provision-host.sh`,
`scripts/test-bootstrap-vps.sh`, possivelmente fixtures shell/Bats apenas para
helpers puros, `docs/iterations/current-iteration.md`.

**Mudanças:**

- Para usuário existente, comparar login shell, home, senha bloqueada e grupos;
  abortar antes de mutation se qualquer propriedade de segurança for inválida.
- Ler `/etc/subuid` e `/etc/subgid` por usuário e por intervalo; reutilizar
  somente um intervalo válido já pertencente a `pneuma`; rejeitar sobreposição
  ou intervalo incompatível antes de chamar `usermod`.
- Aplicar donos e modos por `install -d`/`chown`/`chmod` explícitos e validar
  linger após habilitar.
- Não assumir que a presença de uma linha por nome é suficiente para considerar
  subids corretos.

**Política de ranges fixada:** cada `/etc/subuid` e `/etc/subgid` deve conter
exatamente uma alocação `pneuma:start:count` decimal, com `start > 0` e
`count >= 65536`. O intervalo semiaberto não pode sobrepor uma alocação de
outro usuário no mesmo arquivo; offsets distintos para subuid e subgid são
válidos. Na ausência da entrada, criar somente o range canônico
`100000:65536`, depois de provar que ele não conflita. Entrada malformada,
duplicada, sobreposta ou menor deve falhar antes de `usermod`; ranges adjacentes
são válidos. A validação compartilhada é somente leitura e ocorre em ambos os
chamadores antes de pacotes, conta, subids, diretórios, Caddy, fonte ou binário.

**Verificação focal:** banco de casos com host limpo, rerun, usuário com shell
errado, usuário no grupo sudo, faixa já válida, faixa conflitante e diretório com
modo incorreto. Casos destrutivos usam VM descartável ou fixtures de arquivos,
nunca a VPS.

### 5. Caddy atômico e idempotente

**Arquivos previstos:** `scripts/lib/provision-host.sh`,
`scripts/test-bootstrap-vps.sh`, `docs/getting-started.md`,
`docs/iterations/current-iteration.md`.

**Mudanças:**

- Gerar o Caddyfile candidate no diretório de destino; validar com `caddy
  validate --config <candidate> --adapter caddyfile` antes de tocar o ativo.
- Comparar candidate e ativo. Sem mudança, não criar backup nem reload
  desnecessário. Com mudança, copiar backup datado, instalar com rename atômico
  e reload; erro de validate conserva ativo.
- A checagem de listener do checkpoint 1 trata Caddy ativo como estado conhecido
  apenas quando o Caddyfile gerenciado está válido.

**Verificação focal:** primeiro bootstrap, rerun sem novo backup, candidate
inválido que preserva conteúdo ativo e Caddy saudável, e listener de porta
ocupada por processo não-Caddy que falha com diagnóstico.

### 6. Acceptance de host limpo e rerun

**Arquivos previstos:** `scripts/test-bootstrap-vps.sh`, documentação
operacional, `docs/iterations/current-iteration.md`.

**Procedimento VM:**

1. Criar clone descartável a partir de Debian 13 limpo, nunca de `pneuma-ready`.
2. Gerar localmente a chave CI efêmera; transferir somente a pública.
3. Rodar bootstrap da URL pública com tag ou SHA completo conhecido.
4. Executar todas as assertions de host e registrar PASS/FAIL.
5. Rodar exatamente o mesmo comando outra vez e registrar assertions de rerun.
6. Desligar e destruir a VM em sucesso ou falha; manter logs fora do repositório.

**Verificação focal:** acceptance sem skips. Senha root, se necessária para o
wrapper local, permanece somente em `PNEUMA_VM_ROOT_PASSWORD`, jamais em scripts,
argumentos, logs, VM ou tracker.

### 7. Lint shell no CI

**Arquivos previstos:** `.github/workflows/ci.yml`, todos os `*.sh` que exigirem
formatação/lint, documentação de desenvolvimento, tracker.

**Mudanças:**

- Adicionar job `shell` separado do job Rust, cobrindo todos os scripts via
  `bash -n`, ShellCheck e `shfmt --diff`.
- Fixar versões exatas das ferramentas em setup reproduzível no workflow; não
  depender da versão preinstalada de `ubuntu-latest`.
- Corrigir apenas violações reais; suppressions ShellCheck permanecem locais e
  justificadas quando não houver alternativa segura.

**Verificação focal:** reproduzir os três comandos localmente com as versões
fixadas ou registrar indisponibilidade local; CI deve falhar para script com erro
de sintaxe, warning ShellCheck aplicável ou formatação divergente.

### 8. Candidate falho e rollback real

**Arquivos previstos:** `scripts/dev-vm/e2e.sh`, `scripts/dev-vm/test-all.sh`,
fixtures quando necessário, tutorial VM, tracker.

**Mudanças:**

- Construir v1 saudável, publicar/deployar; publicar candidate v2 que falha no
  health check no mesmo repositório permitido; exigir deployment Failed,
  `pneuma app status` Running para v1 e body v1 no endpoint existente.
- Publicar/deployar v2 saudável, então chamar `pneuma deployment rollback
  healthy-http` em vez de reconstruir v1 e usar novo deploy OCI.
- Verificar status Succeeded do rollback, body v1, novo deployment do tipo
  rollback e provenance/histórico esperado quando exposto pela CLI.

**Verificação focal:** todos os comandos que constituem assertion devem ter exit
status checado; `|| true` permanece apenas em cleanup explicitamente
best-effort.

### 9. Reboot comprovado

**Arquivos previstos:** `scripts/dev-vm/e2e.sh`, `scripts/dev-vm/test-all.sh`,
tutorial VM, tracker.

**Mudanças:**

- Capturar `boot_id` antes do reboot via SSH.
- Solicitar reboot, provar pelo menos uma falha de conexão após a solicitação,
  esperar conexão voltar dentro de timeout e exigir `boot_id` diferente.
- Depois do boot, verificar `user@<uid>.service`, unidade Quadlet ativa,
  container ativo, `pneuma app status` Running e body esperado.

**Verificação focal:** timeout e logs diagnósticos para cada etapa; nunca aceitar
o host que nunca caiu como reboot bem-sucedido.

### 10. HTTPS local e fronteiras CI

**Arquivos previstos:** `scripts/dev-vm/provision-host.sh` ou biblioteca comum,
`scripts/dev-vm/e2e.sh`, `scripts/dev-vm/test-all.sh`, tutorial VM, tracker.

**Mudanças HTTPS:**

- Configurar o Caddyfile da VM com bloco global `local_certs` sem levar essa
  política ao bootstrap de produção.
- Mapear `redirect-public.pneuma.test` e demais domínios fixture usados para
  `127.0.0.1` em `/etc/hosts` da VM.
- Copiar a CA local gerada pelo Caddy ao trust store e executar a atualização de
  certificados antes da assertion HTTPS.
- Exigir deploy público, fragmento presente, health externo/HTTPS funcional;
  depois `visibility set internal`, exigir fragmento ausente e runtime local
  ainda saudável.

**Mudanças SSH:**

- Cobrir chave CI para `version` e `deploy <app> staging` permitidos.
- Exigir rejeição de `id`, `podman ps`, leitura de arquivo, injection em branch,
  comando vazio, `ssh -tt`, port forwarding, agent forwarding e X11 forwarding.
- Cada rejeição verifica que não abriu shell nem executou efeito adicional.

**Verificação focal:** este checkpoint substitui o SKIP atual de
`redirect-public`; resultado final não aceita skip para HTTPS ou dispatcher.

### 11. Restore semântico, documentação e regressão final

**Arquivos previstos:** `scripts/dev-vm/test-all.sh`, documentação operacional,
`README.md`, `docs/architecture/architecture.md` quando comportamento descrito
mudar, tracker.

**Mudanças:**

- Criar `e2e-before-backup`, executar backup, criar `e2e-after-backup`, restaurar
  e afirmar que apenas o primeiro system existe.
- Não reutilizar o backup para uma assertion que apenas confirma exit code.
- Atualizar documentos para distinguir VM bootstrap descartável, VM E2E,
  precondições de chave/registry/local TLS, smoke de produção e comandos de
  regressão.

**Fechamento:** executar quatro gates Rust, `bash -n`, ShellCheck, shfmt,
acceptance VM limpa e `test-all.sh` integral. Registrar contagens reais
PASS/FAIL/SKIP; qualquer skip só é aceitável mediante aprovação explícita antes
do commit de encerramento.

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
