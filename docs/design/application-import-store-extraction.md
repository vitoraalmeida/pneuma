# Design — Extração de persistência de application_import

**Status:** design aprovado para o checkpoint de extração de persistência.

**Base:** `589970a` (`refactor(runtime): materialize image digest as runtime identity`)

**Design aprovado:** [`design/pre-v0.3-consolidation.md`](pre-v0.3-consolidation.md) (Passo 7)

## Objetivo

Extrair todo SQL inline de `application_import.rs` para `application_store.rs`, preservando o comportamento create-only/idempotente e corrigindo o retorno de estado real em reimports de Applications já deployadas.

## Escopo

- Mover geração de IDs, criação de system/application, e inserts de specs para o store.
- Adicionar `load_application_for_import(&Transaction, name)` com `LEFT JOIN application_sources` e `active_deployment_id`.
- Tipar APIs do store para receber `DeliveryType` e `Visibility`, não strings.
- Corrigir reimport para retornar estado real da Application existente.
- Adicionar cobertura de atomicidade, OCI sem source, e reimport deployado.

## Non-goals

- Não alterar o contrato de importação (URLs Git apenas).
- Não alterar schema ou migrations.
- Não implementar update de specs divergentes em reimport.
- Não alterar outros use cases neste checkpoint.

## Alterações no use case

Em `src/use_cases/application_import.rs`:

- Manter `load_manifest_at`, seleção de `DEFAULT_MANIFEST_PATH` e prioridade de `--system`.
- Manter `SystemRequired`.
- Manter `connection.transaction()` e `commit()` no use case.
- Substituir todos os `query_row` e `execute` por chamadas ao `application_store`.
- Remover `persist_specification`.
- Converter `ApplicationStoreError` em `ImportError::Persistence`, preservando o `rusqlite::Error` subjacente.
- Continuar classificando `repository_url` com `is_remote_repository`; esse é contexto de importação, não decisão SQL.

Fluxo:

1. Carregar e validar `pneuma.toml`.
2. Resolver nome de system:
   - `--system` vence `[system].name`.
   - Ausência de ambos retorna `SystemRequired` antes da transação.
3. Abrir uma única transação.
4. Carregar Application existente pelo nome usando `load_application_for_import`.
5. Se já existe:
   - Retornar a Application persistida sem alterar specs ou criar system.
6. Se não existe:
   - Gerar ID de system e chamar `ensure_system`.
   - Carregar ID efetivo do system pelo nome.
   - Gerar ID de Application e chamar `insert_application`.
   - Persistir delivery.
   - Persistir source somente quando `repository_url` existir.
   - Persistir runtime spec.
   - Persistir health spec.
   - Persistir exposure.
7. Carregar a Application pelo nome usando o store.
8. Commit e retorno.

## Alterações no store

Em `src/adapters/stores/application_store.rs`:

- Reutilizar primitives existentes de geração de ID, system, Application e inserts de specs.
- Alterar APIs para receber tipos de domínio:
  - `insert_delivery_spec(..., DeliveryType, ...)`
  - `insert_exposure(..., Visibility, ...)`
- A conversão para `"oci"`, `"public"` e `"internal"` fica no store via `database_value()`.
- Manter `repository_kind` como string neste checkpoint.

Adicionar:

```rust
pub fn load_application_for_import(
    transaction: &Transaction<'_>,
    name: &str,
) -> Result<Option<Application>, ApplicationStoreError>
```

Query:

```sql
SELECT
    a.id,
    a.system_id,
    a.name,
    s.repository_url,
    s.default_branch,
    a.active_deployment_id
FROM applications AS a
LEFT JOIN application_sources AS s
    ON s.application_id = a.id
WHERE a.name = ?1
```

- `LEFT JOIN` preserva imports OCI sem `application_sources`.
- `active_deployment_id` deve ser carregado, não preenchido artificialmente com `None`.
- Ausência é `Ok(None)`, não ID inventado ou `QueryReturnedNoRows`.

## Correção de comportamento

Hoje, ao reimportar uma Application já deployada:

- `application_import` retorna `active_deployment_id: None`, apesar de o banco ter um deployment ativo.
- `main.rs` imprime sempre `Deployment: Not deployed` depois de importar.

Correção:

- `load_application_for_import` retorna o `active_deployment_id` real.
- O fluxo CLI deve derivar a linha final do estado retornado:
  - sem deployment ativo: `Deployment: Not deployed`;
  - com deployment ativo: informar que já está deployada, sem inventar um novo estado.

## Testes

Em `tests/application_import.rs`:

1. **Import remoto OCI sem source duplicado**
   - Importar `oci-only` duas vezes com `repository_url` remoto.
   - Confirmar uma Application, uma delivery spec e uma única `application_sources`.
   - Confirmar que o segundo import não cria specs adicionais (nenhuma linha duplicada).

2. **Reimport de Application deployada**
   - Importar fixture.
   - Criar/persistir deployment ativo e atribuí-lo a `applications.active_deployment_id`.
   - Reimportar o mesmo manifest.
   - Confirmar que o `Application` retornado preserva `Some(deployment_id)`.
   - Confirmar que o source/specs originais continuam inalterados.

3. **Falha no meio do agregado é atômica**
   - Criar trigger temporário que interrompe `INSERT` em `application_runtime_specs`.
   - Importar fixture com URL remota, após delivery/source já terem sido tentados.
   - Esperar `ImportError::Persistence`.
   - Confirmar zero registros em:
     - `systems`
     - `applications`
     - `application_delivery_specs`
     - `application_sources`
     - `application_runtime_specs`
     - `health_check_specs`
     - `exposures`

4. **Reimport divergente é create-only**
   - Reimportar com system, URL ou manifest divergente.
   - Confirmar que a Application existente e seus specs não mudam.
   - Confirmar que o retorno representa a Application persistida, não os argumentos novos.

Em `tests/cli.rs`:

5. **CLI reporta estado real no reimport**
   - Importar e deployar uma Application.
   - Reexecutar `pneuma app import <file-url>`.
   - Confirmar saída correspondente a Application já deployada, em vez de `Deployment: Not deployed`.

## Ordem de implementação

1. Adicionar `load_application_for_import`.
2. Tipar delivery e visibility no store.
3. Refatorar `application_import` para usar somente APIs de store.
4. Corrigir o retorno/saída do reimport deployado.
5. Adicionar os testes de atomicidade, OCI sem source e reimport deployado.
6. Confirmar que não resta `query_row`, `execute`, `prepare` ou `params!` em `application_import.rs`.
7. Rodar:
   - `cargo test --test application_import`
   - `cargo test --test cli`
   - quatro gates completos.
8. Atualizar o checkpoint no tracker e criar o commit:

`refactor(store): move application import persistence to application store`

## Critérios de aceite

- `application_import.rs` não contém SQL inline.
- Store recebe `DeliveryType` e `Visibility`, não strings.
- `load_application_for_import` retorna `active_deployment_id` real.
- Reimport não altera specs divergentes.
- Reimport de Application deployada retorna estado real.
- Falha no meio do agregado é atômica.
- CLI reporta estado real no reimport.
- Quatro gates verdes.
