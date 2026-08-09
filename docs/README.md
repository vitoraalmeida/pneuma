# Documentação do Pneuma

Índice dos documentos e seus status. Regra: um documento **vivo** descreve o
sistema ou o trabalho atual e deve ser atualizado na mesma mudança que o
altera; um **registro** descreve algo executado e não muda.

## Vivos

| Documento | Conteúdo |
|---|---|
| [`rust-guidelines.md`](rust-guidelines.md) | Convenções obrigatórias de código Rust |
| [`usage-guide.md`](usage-guide.md) | Guia passo a passo de uso numa VPS nova: bootstrap, deploy, operação |
| [`architecture/architecture.md`](architecture/architecture.md) | Arquitetura implementada: estrutura, runtime Quadlet, exposição, persistência e máquina de estados |
| [`product/v0.1-scope.md`](product/v0.1-scope.md) | Contrato da v0.1: capacidades, aceite final, não objetivos |
| [`iterations/current-iteration.md`](iterations/current-iteration.md) | Iteração em andamento (o único acompanhamento de trabalho) |
| [`roadmap.md`](roadmap.md) | Roadmap consolidado v0.1 → v0.6; contrato de evolução do projeto |

## Registros operacionais

| Documento | Conteúdo |
|---|---|
| [`operations/staging-validation.md`](operations/staging-validation.md) | Validação manual do contrato do site em staging (ago/2026) |
| [`operations/public-deployment.md`](operations/public-deployment.md) | Procedimento e pré-requisitos do deployment público no host |
| [`operations/vps-bootstrap.md`](operations/vps-bootstrap.md) | Bootstrap de VPS limpa (Debian 13) com Quadlet e GHCR (ago/2026) |
| [`operations/backup-and-restore.md`](operations/backup-and-restore.md) | Backup consistente e recuperação do banco SQLite |
