# Plano de Implementação — Refatoração do `deployment_deploy_release`

**Status:** planejamento
**Criado em:** 10 de agosto de 2026
**Baseado em:** `/home/xpectre/Downloads/deploy-release-refactoring-plan.md`

## Visão Geral

Refatorar `src/use_cases/deployment_deploy_release.rs` (1259 linhas) em módulos coesos, extraindo responsabilidades que mudam por motivos diferentes. O objetivo é que o arquivo original expresse apenas o algoritmo de deployment no nível de aplicação.

**Princípio:** nenhuma mudança de comportamento. Estados, códigos de erro, semântica de rollback, operações de banco e API pública permanecem equivalentes.

## Estrutura Final Esperada

```text
src/use_cases/
├── deployment_deploy_release.rs      (~300 linhas) — orquestrador
├── deployment_progress.rs            (~150 linhas) — reporting
├── deployment_runtime_cleanup.rs     (~200 linhas) — cleanup de candidates e runtimes antigos
├── deployment_start_candidate.rs     (~250 linhas) — criação do runtime candidato
└── deployment_activate_public.rs     (~250 linhas) — ativação pública (health + Caddy)
```

---

## Commit 1 — Testes de Caracterização

```text
test(deployment): add deploy release characterization coverage
```

### Objetivo

Criar rede de segurança antes de mover lógica. Testar comportamento observável, não implementação interna.

### Arquivo

`tests/deployment_deploy_release.rs`

### Estratégia de Testes

Reutilizar `DeploymentEnvironment` de `tests/cli.rs` com extensões pontuais. Os scripts fake já existentes (`podman`, `systemctl`, `caddy`, `curl`) cobrem os cenários principais.

### Casos de Teste

#### 1.1 Deploy interno saudável

```rust
#[test]
fn internal_deploy_succeeds_when_candidate_is_healthy() {
    // Estado inicial: aplicação importada
    // Ação: deploy com health check retornando 200
    // Verifica:
    //   - deployment status = Succeeded
    //   - application.active_deployment_id aponta para o novo deployment
    //   - runtime instances tem entry com state = running
    //   - unit file existe em quadlets/
}
```

#### 1.2 Falha ao iniciar candidate (systemctl start falha)

```rust
#[test]
fn deploy_fails_when_systemctl_start_fails() {
    // Setup: script fake systemctl que falha em `start`
    // Verifica:
    //   - deployment status = Failed
    //   - código de erro = runtime_start_failed
    //   - unit file removido
    //   - porta liberada (runtime_port_reservations vazia)
    //   - nenhum runtime registrado
}
```

**Extensão necessária:** adicionar flag `PNEUMA_FAKE_SYSTEMCTL_START_FAILURE` ao script fake.

#### 1.3 Candidate unhealthy (health check falha)

```rust
#[test]
fn deploy_fails_when_internal_health_check_fails() {
    // Setup: TcpListener que retorna 500
    // Verifica:
    //   - deployment status = Failed
    //   - código = health_check_failed
    //   - candidate runtime marcado como missing
    //   - unit removido
    //   - container removido
    //   - porta liberada
}
```

#### 1.4 Substituição de runtime

```rust
#[test]
fn new_deploy_removes_previous_runtime() {
    // Estado inicial: deploy A bem-sucedido
    // Ação: deploy B
    // Verifica:
    //   - runtime B = Current
    //   - runtime A marcado como removed_at NOT NULL
    //   - unit de A removido
    //   - container de A removido (PNEUMA_FAKE_PODMAN_REMOVED)
}
```

**Extensão necessária:** já suportado via `PNEUMA_FAKE_PODMAN_STALE_ID`.

#### 1.5 Public deploy saudável

```rust
#[test]
fn public_deploy_succeeds_with_caddy_and_external_health() {
    // Setup: aplicação pública (fixture another/)
    // Verifica:
    //   - deployment status = Succeeded
    //   - fragmento Caddy criado em managed_caddy_directory
    //   - exposure status = active
    //   - curl foi chamado com --resolve
}
```

**Extensão necessária:** verificar `PNEUMA_FAKE_CURL_LOG` para `--resolve`.

#### 1.6 Falha após Caddy alterado (external health falha)

```rust
#[test]
fn public_deploy_rolls_back_caddy_when_external_health_fails() {
    // Setup: PNEUMA_FAKE_CURL_STATUS=500
    // Verifica:
    //   - deployment status = Failed
    //   - código = external_health_check_failed
    //   - fragmento Caddy anterior restaurado
    //   - caddy reload chamado novamente
    //   - exposure status = failed
}
```

#### 1.7 Runtime já promovido não deve ser destruído

```rust
#[test]
fn cleanup_does_not_remove_already_promoted_runtime() {
    // Cenário: promoção pública com resultado ambíguo
    // (runtime já é Current quando cleanup é executado)
    // Verifica:
    //   - runtime NÃO é removido
    //   - unit NÃO é removido
    //   - porta NÃO é liberada
}
```

**Nota:** Este cenário é difícil de simular em teste E2E porque requer injeção de falha em momento específico. Alternativa: teste unitário que chama `cleanup_candidate` diretamente com runtime em estado `current`.

### Extensões aos Scripts Fake

Adicionar ao script `systemctl` fake:

```sh
start)
    if [ -f "${PNEUMA_FAKE_SYSTEMCTL_START_FAILURE:-}" ]; then
        printf 'start failed\n' >&2
        exit 1
    fi
    # ... existing logic
```

### Validação

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --test deployment_deploy_release
cargo test  # todos os testes
```

---

## Commit 2 — Extrair Progress Reporting

```text
refactor(deployment): extract progress reporting
```

### Arquivo Novo

`src/use_cases/deployment_progress.rs`

### Tipos Movidos

```rust
// De deployment_deploy_release.rs para deployment_progress.rs:

pub enum DeploymentStep { ... }
pub enum DeploymentProgress { ... }
impl fmt::Display for DeploymentStep { ... }
impl fmt::Display for DeploymentProgress { ... }
pub(crate) struct ProgressReporter<'a> { ... }
```

### Mudanças em `deployment_deploy_release.rs`

1. Remover definições acima
2. Adicionar import:
   ```rust
   use crate::use_cases::deployment_progress::{
       DeploymentProgress, DeploymentStep, ProgressReporter,
   };
   ```
3. Para compatibilidade retroativa (se algum consumidor externo usa esses tipos):
   ```rust
   pub use crate::use_cases::deployment_progress::{
       DeploymentProgress, DeploymentStep,
   };
   ```

### Mudanças em `src/use_cases/mod.rs`

```rust
pub mod deployment_progress;  // adicionar
```

### Validação

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

**Nenhum teste deve falhar.** Este commit é puramente mecânico.

---

## Commit 3 — Extrair Runtime Cleanup

```text
refactor(deployment): extract runtime cleanup
```

### Arquivo Novo

`src/use_cases/deployment_runtime_cleanup.rs`

### Tipos e Funções Movidos

```rust
// Movidos de deployment_deploy_release.rs:

pub(crate) struct PreviousRuntime {
    pub runtime_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
}

pub(crate) fn load_previous_runtime(
    connection: &Connection,
    application_id: &str,
    candidate_runtime_id: &str,
) -> Result<Option<PreviousRuntime>, rusqlite::Error> { ... }

pub(crate) fn retire_previous_runtime(
    connection: &Connection,
    specification: &DeploymentSpecification,
    previous: Option<&PreviousRuntime>,
) { ... }
// (renomeado de finalize_runtime_supervision)

pub(crate) fn cleanup_failed_candidate(
    connection: &Connection,
    deployment_id: &str,
    unit: Option<&str>,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
) -> Result<(), CandidateCleanupError> { ... }
// (renomeado de cleanup_candidate)

pub enum CandidateCleanupError { ... }
impl fmt::Display for CandidateCleanupError { ... }
impl Error for CandidateCleanupError { ... }
```

### Mudanças em `deployment_deploy_release.rs`

1. Remover definições acima
2. Adicionar imports:
   ```rust
   use crate::use_cases::deployment_runtime_cleanup::{
       CandidateCleanupError, PreviousRuntime, cleanup_failed_candidate,
       load_previous_runtime, retire_previous_runtime,
   };
   ```
3. Atualizar chamadas:
   - `finalize_runtime_supervision(...)` → `retire_previous_runtime(...)`
   - `cleanup_candidate(...)` → `cleanup_failed_candidate(...)`

### Dependência

`retire_previous_runtime` e `cleanup_failed_candidate` precisam de `DeploymentSpecification`. Duas opções:

**Opção A:** Mover `DeploymentSpecification` para `deployment_runtime_cleanup.rs`
**Opção B:** Passar apenas os campos necessários (application_name, application_id)

**Recomendação:** Opção B para manter coesão. `DeploymentSpecification` pertence ao orchestrator.

Assinatura ajustada:
```rust
pub(crate) fn retire_previous_runtime(
    connection: &Connection,
    application_name: &str,
    previous: Option<&PreviousRuntime>,
) { ... }
```

### Mudanças em `src/use_cases/mod.rs`

```rust
pub mod deployment_runtime_cleanup;  // adicionar
```

### Validação

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

---

## Commit 4 — Modelar CandidateResources

```text
refactor(deployment): model candidate resources for cleanup
```

### Mudança de Modelagem

Criar tipo que agrupa recursos parcialmente materializados:

```rust
// Em deployment_deploy_release.rs (ou em deployment_runtime_cleanup.rs se fizer sentido)

pub(crate) struct CandidateResources {
    pub unit_name: Option<String>,
    pub container_id: Option<String>,
    pub runtime_id: Option<String>,
    pub port_reserved: bool,
}
```

### Mudança em `FailedExecution`

De:
```rust
struct FailedExecution {
    code: &'static str,
    source: Box<dyn Error>,
    container_id: Option<String>,
    runtime_id: Option<String>,
    failure_persisted: bool,
    unit_name: Option<String>,
    port_reserved: bool,
}
```

Para:
```rust
struct FailedExecution {
    code: &'static str,
    source: Box<dyn Error>,
    failure_persisted: bool,
    resources: CandidateResources,
}
```

### Atualização de Funções Helper

```rust
fn failure_needing_persistence(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
) -> FailedExecution {
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: false,
        resources: CandidateResources {
            unit_name: None,
            container_id: container_id.map(str::to_owned),
            runtime_id: runtime_id.map(str::to_owned),
            port_reserved: false,
        },
    }
}

fn candidate_failure(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
    unit_name: Option<&str>,
    port_reserved: bool,
) -> FailedExecution {
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: false,
        resources: CandidateResources {
            unit_name: unit_name.map(str::to_owned),
            container_id: container_id.map(str::to_owned),
            runtime_id: runtime_id.map(str::to_owned),
            port_reserved,
        },
    }
}

fn failure_already_persisted(
    code: &'static str,
    source: impl Error + 'static,
    container_id: &str,
    runtime_id: &str,
) -> FailedExecution {
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: true,
        resources: CandidateResources {
            unit_name: None,
            container_id: Some(container_id.to_owned),
            runtime_id: Some(runtime_id.to_owned),
            port_reserved: false,
        },
    }
}
```

### Atualização de `finish_failed_deployment`

```rust
fn finish_failed_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    failed: FailedExecution,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeployedRelease, DeployReleaseError> {
    let failure = failed.source.to_string();
    let record_error = if failed.failure_persisted {
        progress.failure_persisted(deployment_id, failed.code);
        None
    } else {
        match fail_deployment(connection, deployment_id, failed.code, &failure) {
            Ok(_) => {
                progress.failure_persisted(deployment_id, failed.code);
                None
            }
            Err(source) => Some(source),
        }
    };
    let cleanup_error = if failed.resources.container_id.is_some()
        || failed.resources.unit_name.is_some()
        || failed.resources.port_reserved
    {
        progress.started(
            DeploymentStep::CleanupCandidate,
            format!("deployment {deployment_id}"),
        );
        match cleanup_failed_candidate(
            connection,
            deployment_id,
            failed.resources.unit_name.as_deref(),
            failed.resources.container_id.as_deref(),
            failed.resources.runtime_id.as_deref(),
        ) {
            Ok(()) => {
                progress.completed(
                    DeploymentStep::CleanupCandidate,
                    format!("deployment {deployment_id}"),
                );
                None
            }
            Err(source) => Some(source),
        }
    } else {
        None
    };

    // ... resto da função
}
```

### Atualização de `execute_deployment`

```rust
let (runtime_id, finished_at) = execute_candidate(...)
    .map_err(|mut failed| {
        failed.resources.unit_name = Some(unit);
        failed.resources.port_reserved = true;
        failed
    })?;
```

### Validação

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

---

## Commit 5 — Extrair Candidate Startup

```text
refactor(deployment): extract candidate startup
```

### Arquivo Novo

`src/use_cases/deployment_start_candidate.rs`

### Responsabilidade

Materializar uma Release como um runtime candidato registrado e pronto para verificação.

### Sequência Movida

```text
reserve_port
    ↓
write_unit
    ↓
daemon_reload
    ↓
start
    ↓
resolve_container_id
    ↓
observe_container
    ↓
register_candidate_runtime
    ↓
consume_port_reservation
    ↓
advance_deployment(Start)
    ↓
advance_deployment(RuntimeRunning)
```

### Tipos e Funções

```rust
// Em deployment_start_candidate.rs

pub(crate) struct StartedCandidate {
    pub runtime: CandidateRuntime,
    pub container_name: String,
    pub unit_name: String,
    pub port: u16,
}

pub(crate) struct CandidateStartInput<'a> {
    pub connection: &'a mut Connection,
    pub deployment_id: &'a str,
    pub application_id: &'a str,
    pub application_name: &'a str,
    pub image_reference: &'a str,
    pub container_port: u16,
    pub source_revision: &'a str,
}

pub(crate) enum CandidateStartError {
    PortAllocation { source: PortAllocationError },
    UnitCreation { source: QuadletError, resources: CandidateResources },
    UnitReload { source: QuadletError, resources: CandidateResources },
    UnitStart { source: QuadletError, resources: CandidateResources },
    ContainerResolution { source: Box<dyn Error>, resources: CandidateResources },
    ContainerObservation { source: Box<dyn Error>, resources: CandidateResources },
    RuntimeRegistration { source: Box<dyn Error>, resources: CandidateResources },
    PortPersistence { source: rusqlite::Error, resources: CandidateResources },
    DeploymentTransition { source: TransitionDeploymentError, resources: CandidateResources },
}

pub(crate) fn start_candidate(
    input: CandidateStartInput<'_>,
) -> Result<StartedCandidate, CandidateStartError> { ... }
```

### Mapeamento de Erros

Os códigos de erro existentes devem ser preservados:

| Erro | Código |
|------|--------|
| `PortAllocation` | `runtime_port_allocation_failed` |
| `UnitCreation` | `runtime_unit_creation_failed` |
| `UnitReload` | `runtime_unit_reload_failed` |
| `UnitStart` | `runtime_start_failed` |
| `ContainerResolution` | `runtime_resolution_failed` |
| `ContainerObservation` | `runtime_observation_failed` |
| `RuntimeRegistration` | `runtime_registration_failed` |
| `PortPersistence` | `runtime_port_persistence_failed` |

### Mudanças em `deployment_deploy_release.rs`

1. Remover lógica de `execute_deployment` que cria o candidate
2. Adicionar import:
   ```rust
   use crate::use_cases::deployment_start_candidate::{
       CandidateStartError, CandidateStartInput, start_candidate,
   };
   ```
3. Simplificar `execute_deployment`:
   ```rust
   fn execute_deployment(
       connection: &mut Connection,
       deployment_id: &str,
       specification: &DeploymentSpecification,
       image_reference: &str,
       source_revision: &str,
       public_configuration: Option<&PublicDeploymentConfiguration>,
       progress: &mut ProgressReporter<'_>,
   ) -> Result<(String, String, String), FailedExecution> {
       advance_deployment(connection, deployment_id, DeploymentTransition::Start)
           .map_err(|source| failure_needing_persistence("deployment_transition_failed", source, None, None))?;
       progress.state_changed(deployment_id, DeploymentStatus::Starting);

       progress.started(DeploymentStep::CreateContainer, format!("image {image_reference}"));
       let input = CandidateStartInput {
           connection,
           deployment_id,
           application_id: &specification.application_id,
           application_name: &specification.application_name,
           image_reference,
           container_port: specification.container_port,
           source_revision,
       };
       let candidate = start_candidate(input).map_err(|err| match err {
           CandidateStartError::PortAllocation { source } => {
               failure_needing_persistence("runtime_port_allocation_failed", source, None, None)
           }
           CandidateStartError::UnitCreation { source, resources } => {
               FailedExecution {
                   code: "runtime_unit_creation_failed",
                   source: Box::new(source),
                   failure_persisted: false,
                   resources,
               }
           }
           // ... outros casos
       })?;
       progress.completed(
           DeploymentStep::CreateContainer,
           format!("unit {}, endpoint 127.0.0.1:{}", candidate.unit_name, candidate.port),
       );
       progress.completed(
           DeploymentStep::StartContainer,
           format!("container {}", candidate.runtime.external_runtime_id),
       );
       progress.completed(
           DeploymentStep::ObserveContainer,
           format!("state Running, endpoint {}", candidate.runtime.endpoint),
       );
       progress.completed(
           DeploymentStep::RegisterCandidate,
           format!("runtime {}", candidate.runtime.id),
       );
       progress.state_changed(deployment_id, DeploymentStatus::Verifying);

       let previous_runtime = load_previous_runtime(
           connection,
           &specification.application_id,
           &candidate.runtime.id,
       ).map_err(|source| {
           // ... erro
       })?;

       // Continuar com promote_internal ou activate_public
       // ...
   }
   ```

### Validação

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

---

## Commit 6 — Extrair Public Activation

```text
refactor(deployment): extract public activation flow
```

### Arquivo Novo

`src/use_cases/deployment_activate_public.rs`

### Responsabilidade

Coordenar a transformação de um candidate saudável em um runtime publicamente acessível.

### Sequência Movida

```text
internal_health
    ↓
advance_deployment(Verified)
    ↓
begin_public_exposure
    ↓
materialize_caddy_fragment
    ↓
external_health
    ↓
promote_public_candidate
```

Com compensação:
```text
Caddy alterado + external_health falha → restore_materialized_caddy_fragment
```

### Tipos e Funções

```rust
// Em deployment_activate_public.rs

pub(crate) struct PublicActivationInput<'a> {
    pub connection: &'a mut Connection,
    pub runtime: &'a CandidateRuntime,
    pub application_id: &'a str,
    pub health_path: &'a str,
    pub expected_status: u16,
    pub source_revision: &'a str,
    pub managed_caddy_directory: &'a Path,
    pub caddyfile_path: &'a Path,
}

pub(crate) struct PublicActivationResult {
    pub finished_at: String,
}

pub(crate) enum PublicActivationError {
    InternalHealth { source: Box<dyn Error>, resources: CandidateResources },
    DeploymentTransition { source: TransitionDeploymentError, resources: CandidateResources },
    ExposurePreparation { source: Box<dyn Error>, resources: CandidateResources },
    CaddyMaterialization { source: Box<dyn Error>, outcome: ExposureOutcome, resources: CandidateResources },
    ExternalHealth { source: Box<dyn Error>, outcome: ExposureOutcome, resources: CandidateResources },
    Promotion { source: Box<dyn Error>, outcome: ExposureOutcome, resources: CandidateResources },
}

pub(crate) fn activate_public_candidate(
    input: PublicActivationInput<'_>,
) -> Result<PublicActivationResult, PublicActivationError> { ... }
```

### Tipos Movidos

```rust
// Movidos de deployment_deploy_release.rs:

pub(crate) struct PublicHealthFailure { ... }
pub(crate) struct PublicRouteRollbackError { ... }
pub(crate) struct ExposureFailureRecordingError { ... }

fn rollback_public_route(...) -> (Box<dyn Error>, ExposureOutcome) { ... }
fn public_failure(...) -> FailedExecution { ... }
```

### Mudanças em `deployment_deploy_release.rs`

1. Remover `execute_public_candidate` e tipos relacionados
2. Adicionar import:
   ```rust
   use crate::use_cases::deployment_activate_public::{
       PublicActivationError, PublicActivationInput, activate_public_candidate,
   };
   ```
3. Simplificar branch pública em `execute_candidate`:
   ```rust
   if specification.visibility == Visibility::Public {
       let Some(public_configuration) = public_configuration else {
           return Err(failure_needing_persistence(
               "public_configuration_missing",
               DeployReleaseError::PublicApplication {
                   application_id: specification.application_id.clone(),
               },
               Some(container_id),
               Some(runtime_id),
           ));
       };
       progress.started(DeploymentStep::InternalHealthCheck, ...);
       let input = PublicActivationInput {
           connection,
           runtime: &runtime,
           application_id: &specification.application_id,
           health_path: &specification.health_path,
           expected_status: specification.expected_status,
           source_revision: commit_sha,
           managed_caddy_directory: &public_configuration.managed_caddy_directory,
           caddyfile_path: &public_configuration.caddyfile_path,
       };
       let result = activate_public_candidate(input).map_err(|err| {
           // Converter PublicActivationError para FailedExecution
           // ...
       })?;
       progress.completed(DeploymentStep::PromoteCandidate, ...);
       progress.state_changed(deployment_id, DeploymentStatus::Succeeded);
       finalize_runtime_supervision(connection, specification, previous_runtime.as_ref());
       return Ok((runtime.id, result.finished_at));
   }
   ```

### Validação

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

---

## Commit 7 — Simplificar Orquestração

```text
refactor(deployment): simplify deploy release orchestration
```

### Objetivo

Com todas as responsabilidades extraídas, simplificar `deployment_deploy_release.rs` para expressar apenas o algoritmo de deployment.

### Estrutura Final Esperada

```rust
pub fn deploy_release(...) -> Result<DeployedRelease, DeployReleaseError> {
    let mut progress = ProgressReporter::disabled();
    deploy_release_reporting(..., &mut progress)
}

pub fn deploy_release_with_progress(...) -> Result<DeployedRelease, DeployReleaseError> {
    let mut progress = ProgressReporter::enabled(progress);
    deploy_release_reporting(..., &mut progress)
}

fn deploy_release_reporting(...) -> Result<DeployedRelease, DeployReleaseError> {
    // 1. Load specification
    progress.started(DeploymentStep::LoadSpecification, ...);
    let specification = load_specification(connection, application_id)?;
    progress.completed(DeploymentStep::LoadSpecification, ...);

    if specification.visibility == Visibility::Public && public_configuration.is_none() {
        return Err(DeployReleaseError::PublicApplication { ... });
    }

    // 2. Create deployment
    progress.started(DeploymentStep::CreateDeployment, ...);
    let deployment = create_deployment(connection, application_id, &release.id, deployment_type)?;
    progress.completed(DeploymentStep::CreateDeployment, ...);
    progress.state_changed(&deployment.id, DeploymentStatus::Pending);

    // 3. Execute deployment
    let execution = execute_deployment(...);
    match execution {
        Ok((runtime_id, container_name, finished_at)) => Ok(DeployedRelease { ... }),
        Err(failed) => finish_failed_deployment(connection, &deployment.id, failed, progress),
    }
}

fn execute_deployment(...) -> Result<(String, String, String), FailedExecution> {
    // Transition to Starting
    advance_deployment(connection, deployment_id, DeploymentTransition::Start)?;
    progress.state_changed(deployment_id, DeploymentStatus::Starting);

    // Start candidate
    progress.started(DeploymentStep::CreateContainer, ...);
    let candidate = start_candidate(input).map_err(...)?;
    progress.completed(DeploymentStep::CreateContainer, ...);
    progress.completed(DeploymentStep::StartContainer, ...);
    progress.completed(DeploymentStep::ObserveContainer, ...);
    progress.completed(DeploymentStep::RegisterCandidate, ...);
    progress.state_changed(deployment_id, DeploymentStatus::Verifying);

    // Load previous runtime
    let previous_runtime = load_previous_runtime(...)?;

    // Promote based on visibility
    let result = match specification.visibility {
        Visibility::Internal => {
            progress.started(DeploymentStep::HealthCheckAndPromotion, ...);
            let promoted = promote_internal_candidate(...).map_err(...)?;
            progress.completed(DeploymentStep::HealthCheckAndPromotion, ...);
            progress.state_changed(deployment_id, DeploymentStatus::Succeeded);
            Ok((promoted.finished_at, None))
        }
        Visibility::Public => {
            let input = PublicActivationInput { ... };
            let result = activate_public_candidate(input).map_err(...)?;
            Ok((result.finished_at, Some(...)))
        }
    };

    // Finalize
    match result {
        Ok((finished_at, _)) => {
            retire_previous_runtime(connection, &specification.application_name, previous_runtime.as_ref());
            Ok((candidate.runtime.id, candidate.container_name, finished_at))
        }
        Err(failure) => Err(failure),
    }
}
```

### Tipos que Permanecem

```rust
pub struct DeployedRelease { ... }
pub struct PublicDeploymentConfiguration { ... }
pub enum DeployReleaseError { ... }
impl fmt::Display for DeployReleaseError { ... }
impl Error for DeployReleaseError { ... }

struct DeploymentSpecification { ... }
struct FailedExecution { ... }

fn load_specification(...) -> Result<DeploymentSpecification, DeployReleaseError> { ... }
fn finish_failed_deployment(...) -> Result<DeployedRelease, DeployReleaseError> { ... }
```

### Validação Final

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

---

## Checklist de Validação por Commit

| Commit | fmt | clippy | test | build |
|--------|-----|--------|------|-------|
| 1 | ✓ | ✓ | ✓ | ✓ |
| 2 | ✓ | ✓ | ✓ | ✓ |
| 3 | ✓ | ✓ | ✓ | ✓ |
| 4 | ✓ | ✓ | ✓ | ✓ |
| 5 | ✓ | ✓ | ✓ | ✓ |
| 6 | ✓ | ✓ | ✓ | ✓ |
| 7 | ✓ | ✓ | ✓ | ✓ |

---

## Riscos e Mitigações

### Risco 1: Quebrar compatibilidade de API pública

**Mitigação:** Usar `pub use` para reexportar tipos movidos quando necessário.

### Risco 2: Perder informação de erro durante extração

**Mitigação:** Preservar todos os códigos de erro existentes. Testes de caracterização (Commit 1) garantem que o comportamento observável não muda.

### Risco 3: Dificuldade em passar `Connection` entre módulos

**Mitigação:** Usar structs de input (`CandidateStartInput`, `PublicActivationInput`) que contêm `&mut Connection`. Isso mantém a explícitude das dependências.

### Risco 4: Complexidade excessiva nos tipos de erro

**Mitigação:** Manter enum de erros por módulo, mas converter para `FailedExecution` no orchestrator. Isso mantém a fronteira clara.

---

## Ordem de Execução

1. **Commit 1** — Testes de caracterização (rede de segurança)
2. **Commit 2** — Progress reporting (baixo risco, reduz ruído)
3. **Commit 3** — Runtime cleanup (mover lógica existente)
4. **Commit 4** — CandidateResources (melhoria de modelagem)
5. **Commit 5** — Candidate startup (principal corte estrutural)
6. **Commit 6** — Public activation (segundo corte estrutural)
7. **Commit 7** — Simplificação final (resultado da extração)

Cada commit deve ser autocontido e passar em todos os checks antes de prosseguir.

---

## Critério de Conclusão

A refatoração termina quando `deployment_deploy_release.rs`:

1. Expressa o algoritmo de deployment no nível de aplicação
2. Tem menos de 400 linhas
3. Não contém detalhes de `systemctl`, Quadlet, Caddy ou compensação
4. Passa em todos os testes de caracterização
5. Mantém compatibilidade de API pública

O tamanho reduzido é consequência, não objetivo.

---

## Validação Final — Testes na VM Local

Após completar todos os commits, executar testes de integração na VM local usando os scripts fake existentes para validar o comportamento end-to-end.

### Procedimento

1. **Preparar ambiente de teste:**
   ```bash
   # Criar diretório temporário para os binários fake
   mkdir -p /tmp/pneuma-test/bin
   export PATH="/tmp/pneuma-test/bin:$PATH"
   ```

2. **Instalar scripts fake:**
   - Copiar/adaptar os scripts fake de `tests/cli.rs`:
     - `podman` (fake)
     - `systemctl` (fake)
     - `caddy` (fake)
     - `curl` (fake)
   
   - Garantir que os scripts respeitam as variáveis de ambiente:
     - `PNEUMA_FAKE_PODMAN_LOG`
     - `PNEUMA_FAKE_SYSTEMCTL_START_FAILURE`
     - `PNEUMA_FAKE_CURL_STATUS`
     - `PNEUMA_FAKE_PODMAN_DIGEST`
     - etc.

3. **Executar testes manuais:**
   
   a. **Deploy interno saudável:**
   ```bash
   # Importar aplicação
   ./target/release/pneuma app import tests/fixtures/another
   
   # Iniciar servidor HTTP fake na porta esperada
   # (usar script ou netcat)
   
   # Deploy
   ./target/release/pneuma app deploy another-site --image registry.example/team/service@sha256:$(printf 'a%.0s' {1..64})
   
   # Verificar:
   # - status = Succeeded
   # - runtime registrado
   # - unit file criado
   ```
   
   b. **Falha ao iniciar candidate:**
   ```bash
   export PNEUMA_FAKE_SYSTEMCTL_START_FAILURE=1
   ./target/release/pneuma app deploy another-site --image ...
   # Verificar: status = Failed, código = runtime_start_failed
   unset PNEUMA_FAKE_SYSTEMCTL_START_FAILURE
   ```
   
   c. **Substituição de runtime:**
   ```bash
   # Primeiro deploy
   ./target/release/pneuma app deploy another-site --image sha256:aaa...
   
   # Segundo deploy com digest diferente
   ./target/release/pneuma app deploy another-site --image sha256:bbb...
   
   # Verificar:
   # - runtime anterior marcado como removed
   # - novo runtime = Current
   # - unit anterior removido
   ```
   
   d. **Deploy público:**
   ```bash
   # Configurar aplicação como pública
   ./target/release/pneuma app visibility set another-site public
   
   # Deploy
   ./target/release/pneuma app deploy another-site --image ...
   
   # Verificar:
   # - fragmento Caddy criado
   # - exposure status = active
   # - curl chamado com --resolve
   ```

4. **Validar rollback:**
   ```bash
   # Configurar external health para falhar
   export PNEUMA_FAKE_CURL_STATUS=500
   
   # Deploy público
   ./target/release/pneuma app deploy another-site --image ...
   
   # Verificar:
   # - deployment = Failed
   # - Caddy restaurado para configuração anterior
   # - exposure status = failed
   ```

5. **Limpar ambiente:**
   ```bash
   rm -rf /tmp/pneuma-test
   # Remover database temporário se necessário
   ```

### Critério de Aceite

- Todos os cenários acima executam sem erros inesperados
- Comportamento observável é idêntico ao pré-refatoração
- Logs de warning (quando aplicável) aparecem corretamente
- Nenhum artefato órfão (units, containers, ports) permanece após falhas
