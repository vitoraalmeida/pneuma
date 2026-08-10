# Plano Fase C — Manifest v3 + Schema

**Status:** Aprovado, pronto para implementação  
**Data:** 10 de agosto de 2026  
**Contexto:** Fases A e B concluídas. Próxima: fase C parcial (D fica para depois).

---

## Objetivo

Preparar o schema e o código para o fluxo Git-aware da v0.2:
- Manifest v3 sem `[source]`/`[build]`
- `application_import` recebe `repository_url` e `manifest_path` como parâmetros
- Migration para ajustar `application_sources`
- CLI não muda (isso é fase D)

---

## Mudanças planejadas

### 1. Manifest v3 (`src/domain/manifest.rs`)

- `SUPPORTED_SCHEMA_VERSION = 3`
- Remover `Source` struct
- Remover `source: Option<Source>` do `Manifest`
- Remover `validate_source` e sua chamada em `validate_manifest`

### 2. Migration 0013 (`migrations/0013_application_sources_v3.sql`)

```sql
ALTER TABLE application_sources RENAME COLUMN repository_location TO repository_url;
CREATE TABLE application_sources_new (
    application_id TEXT PRIMARY KEY
        REFERENCES applications(id) ON DELETE CASCADE,
    repository_url TEXT NOT NULL,
    repository_kind TEXT NOT NULL
        CHECK (repository_kind IN ('local', 'remote')),
    default_branch TEXT,
    manifest_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO application_sources_new
    SELECT application_id, repository_url, repository_kind, default_branch, manifest_path, created_at, updated_at
    FROM application_sources;
DROP TABLE application_sources;
ALTER TABLE application_sources_new RENAME TO application_sources;
```

Nota: SQLite não suporta `ALTER COLUMN` para tornar nullable nem `RENAME COLUMN` em versões antigas. A abordagem é recriar a tabela.

### 3. `application_import.rs`

- Mudar assinatura: `import_application(connection, repository_path, system_name)` → `import_application(connection, repository_path, system_name, repository_url, manifest_path)`
- `repository_url: Option<&str>` — se Some, persiste em `application_sources`
- `manifest_path: Option<&str>` — default `"pneuma.toml"`
- Remover leitura de `manifest.source`
- Usar `repository_url` e `manifest_path` dos parâmetros

### 4. CLI (`src/main.rs`)

- `run_import` passa `Some(repository_path.to_str())` como `repository_url` e `Some("pneuma.toml")` como `manifest_path`
- USAGE não muda (fase D muda)

### 5. Fixtures (`tests/fixtures/`)

- `valid/pneuma.toml`: `schema_version = 3`, remover `[source]`
- `oci-only/pneuma.toml`: `schema_version = 3`
- `another/pneuma.toml`: `schema_version = 3`, remover `[source]` se existir

### 6. Tests

- Atualizar `tests/manifest.rs` para v3
- Atualizar `tests/cli.rs` para nova assinatura de `import_application`
- Atualizar `src/adapters/database.rs` tests (migration count)

### 7. Database.rs

- Registrar migration 0013
- Atualizar `MIGRATIONS` array
- Atualizar `migration_count` nos tests

---

## Checklist de implementação

- [ ] Criar `migrations/0013_application_sources_v3.sql`
- [ ] Atualizar `src/adapters/database.rs` (registrar migration, atualizar tests)
- [ ] Atualizar `src/domain/manifest.rs` (v3, remover Source)
- [ ] Atualizar `src/use_cases/application_import.rs` (nova assinatura)
- [ ] Atualizar `src/main.rs` (run_import)
- [ ] Atualizar fixtures (`tests/fixtures/`)
- [ ] Atualizar `tests/manifest.rs`
- [ ] Atualizar `tests/cli.rs`
- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-features`
- [ ] `cargo build --release`

---

## Commits sugeridos

1. **`feat: add migration 0013 for application_sources v3`**
   - Migration para renomear coluna e tornar default_branch nullable

2. **`feat: manifest schema v3`**
   - Remover Source struct e validação
   - Atualizar fixtures

3. **`refactor: application_import receives repository_url and manifest_path`**
   - Nova assinatura, remove leitura de manifest.source
   - Atualizar CLI e tests

---

## Como retomar

1. Ler este plano
2. Criar migration 0013
3. Atualizar database.rs
4. Atualizar manifest.rs
5. Atualizar application_import.rs
6. Atualizar main.rs
7. Atualizar fixtures e tests
8. Rodar os 4 checks
9. Commit na ordem sugerida
