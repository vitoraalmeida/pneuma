# Iteração atual

**Status:** concluída

**Atualizado em:** 11 de agosto de 2026

## Iteração — Correção da bateria e2e (bugs do runtime)

Objetivo: corrigir os dois bugs descobertos na bateria de testes e2e
(`scripts/dev-vm/test-all.sh`) executada na VM de desenvolvimento. Ambos os
bugs corrigidos e verificados; bateria completa: 28 PASS / 0 FAIL.

## Bug 1 — `visibility set internal` falha com NULL em `domain` (CORRIGIDO)

- **Sintoma:** `pneuma app visibility set healthy-http internal` falhava com
  `Invalid column type Null at index: 0, name: domain`.
- **Causa-raiz:** em `src/use_cases/exposure_change.rs`, o caminho idempotente
  (~linha 163) e o `make_internal` (~linha 293) liam `SELECT domain` com
  `row.get(0)`, o que infere `String` e quebra quando o valor é NULL (exposição
  interna não tem domínio).
- **Correção:** `row.get::<_, Option<String>>(0)` seguido de `.flatten()` após
  o `.optional()` nos dois pontos (sem isso vira `Option<Option<String>>`).
- **Teste:** `visibility_set_internal_is_idempotent_without_domain` em
  `tests/cli.rs` (regressão).
- **Verificação:** `cargo fmt --check`, `clippy --all-targets --all-features
  -- -D warnings`, `cargo test --all-features` e `cargo build --release` verdes;
  binary sincronizado com `scripts/dev-vm/sync-binary.sh`; validado ao vivo na
  VM (`Visibility for healthy-http: Internal`); fase 5 da bateria (visibilidade
  idempotente) passou a PASS.

## Bug 2 — `app stop/start/status` falham com "not deployed" após `app stop` (CORRIGIDO)

### Sintoma

Na bateria na VM limpa: `app stop` passa, mas os quatro testes seguintes falham
todos com "not deployed": `app stop` idempotente, `app start`, `app start`
idempotente e `app status`. Resultado da bateria: 24 PASS / 4 FAIL.

### Causa-raiz

1. `app stop` para a unit Quadlet;
2. o `ExecStop` da unit é `podman rm` — o container é **removido**, não apenas
   parado;
3. a observação pós-stop é `Missing`;
4. `persist_observation` (`src/use_cases/application_runtime.rs:381`) grava
   `removed_at = CURRENT_TIMESTAMP` mantendo `state = 'running'`;
5. `load_current_runtime` (linha 313) filtra `removed_at IS NULL` → os comandos
   seguintes devolvem `NotDeployed` ("not deployed");
6. `transition_application` (linha 199) faz short-circuit em `Missing` (linha
   214) e retorna erro antes de conseguir acionar `start_unit`, então
   `app start` não reinicia a unit.

### Correção

1. **`transition_application` (`application_runtime.rs`)** — dois cenários:
   - **Observação inicial `Missing` com `desired_runtime_state == Stopped`:**
     deduzir observação `Stopped` (sem `removed_at`) e persistir como sucesso
     idempotente via `persist_stopped_without_removal`.
   - **Observação inicial `Missing` com `desired_runtime_state == Running`:**
     verificar se a unit Quadlet existe; se existe, acionar `start_unit` e
     re-observar via `observe_current_runtime` (que reconcilia o id do container
     recriado por nome estável).
   - **Observação pós-controle `Missing` com `desired_runtime_state == Stopped`:**
     após `stop_unit` (cujo `ExecStop` remove o container), deduzir observação
     `Stopped` e persistir sem `removed_at`.
2. **`persist_stopped_without_removal`** — nova função que grava
   `last_observed_state` sem gravar `removed_at`, mantendo o runtime carregável
   por `load_current_runtime`.
3. **`runtime_store.rs`** — adicionado helper `set_runtime_state` (não utilizado
   diretamente na correção final, mas disponível para uso futuro).
4. **Não alterado** o path de `report_application_status` (Missing → removed_at
   permanece para `app status` quando o desired não é Stopped).

### Teste

- **`stop_and_start_cycle_after_container_removal_by_quadlet`** em `tests/cli.rs`
  (regressão): ciclo completo stop→start com container removido, verificando
  idempotência e ausência de "not deployed".
- **`a_removed_container_guides_a_new_deployment`** atualizado: `app stop` com
  container removido agora retorna sucesso (comportamento corrigido).

### Verificação

- `cargo fmt --check`, `clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features` e `cargo build --release` verdes.
- Binary sincronizado com `scripts/dev-vm/sync-binary.sh`.
- Bateria completa na VM: **28 PASS / 0 FAIL** (incluindo os 4 testes que
  falhavam: `app stop` idempotente, `app start`, `app start` idempotente e
  `app status`).

## VM de desenvolvimento (pneuma-dev-base)

- **VM nova:** domínio `pneuma-dev-base`, IP **192.168.122.6** (a antiga
  `.19`/`debian13` ficou inacessível — "No route to host"). Acesso via
  `ssh pneuma-dev` (alias em `~/.ssh/config`, HostName 192.168.122.6, User root,
  key `~/.ssh/pneuma-dev`).
- **Provisionamento:** `provision-host.sh` rodado (após desativar o repositório
  `deb cdrom:` em `/etc/apt/sources.list`); pacote `sudo` instalado (os scripts
  usam `sudo chown`/`sudo reboot` e falhavam em VM sem sudo); binary release
  (com o fix do bug 1) instalado em `/usr/local/bin/pneuma`; `pneuma doctor`
  tudo OK.
- **Registry:** insecure configurado em
  `/etc/containers/registries.conf.d/pneuma-dev.conf`
  (`[[registry]] location = "localhost:5000" insecure = true`) — sem isso a
  bateria falhava com "server gave HTTP response to HTTPS client".
- **Caddy:** `local_certs` + `caddy trust` + entradas
  `127.0.0.1 redirect-public.pneuma.test` (e `site`/`api.pneuma.test`) em
  `/etc/hosts`.
- **CI:** chave restrita em `/home/pneuma/.ssh/authorized_keys` com
  `restrict,command="/usr/local/bin/pneuma ci dispatch" ... pneuma-ci-test`.
- **Fixtures:** copiadas para `/var/lib/pneuma/checkouts/fixtures/` com chown
  pneuma + build/push manual no registry (o `rebuild-fixtures.sh` falha no
  `sudo chown` em VM sem sudo).
- **Acesso ao DB/estado na VM:** `runuser -u pneuma -- bash -lc "cd ~ && ..."`
  (evita `cannot chdir to /root`); DB em
  `/var/lib/pneuma/database/pneuma.sqlite3`.

### Anotações operacionais da depuração

- Bateria roda com `bash scripts/dev-vm/test-all.sh pneuma-dev` (não `./...` —
  sem permissão de execução).
- Antes de baterias, salvar baseline do DB em `/tmp/pneuma-baseline-before-battery.sql`.
- Log da bateria com o bug 2: `/tmp/pneuma-test-all-run2.log` (24/4).
- Cadência de ~12s entre remoções de runtime é o ritmo normal de
  `retire_previous_runtime`; não é por si o bug.

## Próximos passos

- [x] Implementar o plano de correção do bug 2 (transição + `set_runtime_state`).
- [x] Adicionar teste de regressão do ciclo stop→start.
- [x] Rodar os quatro checks do `AGENTS.md`.
- [x] `cargo build --release` + `scripts/dev-vm/sync-binary.sh pneuma-dev`.
- [x] Re-rodar `bash scripts/dev-vm/test-all.sh pneuma-dev` e confirmar 28 PASS / 0 FAIL.
