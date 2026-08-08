# Iteração atual

**Status:** concluída

**Atualizado em:** 8 de agosto de 2026

## Iteração — Manifesto v2 com `[delivery]` (entrega 5 da v0.1 OCI)

Objetivo: tornar a entrega OCI declarativa no manifesto, permitindo que `app
deploy --image` valide o repositório da imagem e que `deploy-source` seja um
caminho explícito apenas para aplicações com build local.

### Critérios de aceite

- [x] `pneuma.toml` aceita `schema_version = 2` com seção `[delivery]`
      (`type = "oci"`, `image = <repositório>`), obrigatória.
- [x] `[source]` e `[build]` tornam-se opcionais e devem vir juntos; ausentes
      para aplicações entregues apenas por OCI.
- [x] Manifestos v1 são rejeitados com erro de schema version.
- [x] `application_delivery_specs` é persistida na importação
      (migration `0011_application_delivery_specs`).
- [x] `app deploy --image` rejeita imagem de repositório diferente do permitido
      antes de qualquer trabalho externo.
- [x] `app deploy-source` rejeita aplicação sem `[source]`/`[build]` com
      mensagem clara, antes de Git ou Podman.
- [x] `Application.repository`/`default_branch` tornam-se opcionais; `app list`
      e `system show` continuam funcionando para aplicações OCI-only.
- [x] Fixtures atualizadas para v2 e novo fixture `oci-only`.
- [x] Docs (roadmap, arquitetura, scope) refletem a entrega 5.
- [x] Os quatro checks passam: fmt, clippy `-D warnings`, test `--all-features`,
      build `--release`.
