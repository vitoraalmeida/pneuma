# Iteração atual

**Status:** concluída

**Atualizado em:** 8 de agosto de 2026

## Iteração — Operabilidade final (entrega 7 da v0.1 OCI)

Objetivo: tornar runtimes sobreviventes a reboot via Quadlet e concluir as
operações de recuperação e diagnóstico da v0.1.

### Critérios de aceite

- [x] Deployment reserva porta loopback fixa e gera uma unidade Quadlet por deployment.
- [x] Candidata inicia pelo systemd user manager; a unidade somente é habilitada após promoção.
- [x] `app start` e `app stop` controlam unidades Quadlet, com fallback para runtimes legados.
- [x] Backup e restore SQLite são expostos na CLI com validação e cópia pre-restore.
- [x] Doctor valida Podman rootless e a configuração Caddy.
- [x] Documentação OCI-first, pull de registry, espaço em disco e E2E de reboot no VPS.
