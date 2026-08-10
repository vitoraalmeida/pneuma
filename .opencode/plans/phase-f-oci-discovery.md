# Plano Fase F — OCI Discovery

**Status:** Aprovado, pronto para implementação
**Data:** 10 de agosto de 2026
**Contexto:** Fases A–E concluídas e commitadas (A `fbb62df`, B, C `24432a2`, D `69c2cd5`, E `fadeb11`). Próxima: fase F.

---

## Objetivo

Resolver a tag `image:<commit-sha>` publicada pelo CI em um digest imutável, sem nunca devolver uma tag mutável ao engine.

## Decisões de design (confirmadas com o usuário)

- **Opção A escolhida:** `podman pull --quiet <repo>:<commit_sha>` + `podman image inspect --format {{.Digest}} <repo>:<commit_sha>`.
  - `podman manifest inspect` foi testado na VM e **falha em imagens single-arch** ("Treating single images as manifest lists is not implemented") — plano original invalidado.
  - Não existe `podman pull --dry-run`.
- **Erro genérico sem categorização:** um único `PullError` com stdout/stderr (mais robusto; menos frágil que fazer match em texto do stderr).
- **Atualizar current-iteration.md agora:** marcar fases D e E como `[x]` (além da F).

## Comportamento verificado na VM (pneuma-dev)

- `podman pull -q localhost:5000/healthy-http:latest` + `podman image inspect --format {{.Digest}}` → `sha256:b63028e0bbcef8c2036919ab7c33a67a947eda86a899eae9c35da2c5c0491aa3` (digest do manifesto).
- Tag inexistente: `Error: ... reading manifest <tag> ... manifest unknown`, exit=125.
- Registry inacessível: `Error: ... pinging container registry ... connection refused`, exit=125.
- Ambos exit 125; diferenciação é só por texto do stderr → não categorizar.

## Mudanças planejadas

### 1. `src/adapters/oci_image.rs`

Nova função:

```rust
pub fn resolve_image_digest(
    repository: &str,
    commit_sha: &CommitSha,
) -> Result<OciImageReference, ResolveImageDigestError> {
    let tagged = format!("{repository}:{}", commit_sha.as_str());

    // 1. podman pull --quiet <repository>:<commit_sha>
    // 2. podman image inspect --format {{.Digest}} <repository>:<commit_sha>
    // 3. Parsear output (trim + validar formato sha256:...)
    // 4. Construir OciImageReference::new(repository, &digest)?
}

pub enum ResolveImageDigestError {
    Execute { operation: &'static str, source: io::Error },
    Pull { reference: String, stdout: String, stderr: String },
    Inspect { reference: String, stdout: String, stderr: String },
    InvalidInspectOutput { reference: String, output: String },
}
```

### 2. Testes (`src/adapters/oci_image.rs`)

- `resolve_image_digest_returns_digest_for_existing_tag` (registry local, tag = SHA)
- `resolve_image_digest_fails_for_missing_tag` (tag inexistente)
- `resolve_image_digest_fails_for_unreachable_registry` (registry inacessível)

### 3. `docs/iterations/current-iteration.md`

- Marcar fases D e E como `[x]`

## Observações

- `Cargo.toml` tem `serde` mas **não** `serde_json` — com a Opção A não é preciso adicionar dependência.
- `CommitSha` já existe em `src/adapters/git_source.rs` (exatamente 40 hex chars).
- `OciImageReference` e `pull_image` já existem em `oci_image.rs`.

## Validação na VM

1. `sync-binary.sh` — binário atualizado; doctor verde.
2. Tag `localhost:5000/healthy-http:<commit-sha>` + chamar `resolve_image_digest` → digest correto.
3. Idempotência: chamar 2x, mesmo resultado.

## Checks obrigatórios

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

## Commit

- `feat: resolve OCI image digests from commit tags` (+ commit separado `docs:` para current-iteration.md, se preferido)

## Próximo depois da fase F

Fase G: `deployment_deploy_branch.rs` (branch → CommitSha → digest → `DeployOci`, exclusão mútua `--branch`/`--image`, persistir `source_revision`).
