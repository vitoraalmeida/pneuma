# Iteração atual — Histórico de deployments

**Status:** concluída

**Atualizado em:** 7 de agosto de 2026

**Objetivo:** permitir consultar o histórico de deployments de uma aplicação, exibindo commit, status e timestamp de cada tentativa.

## Trabalho atual — item 37 da sequência de implementação

Adicionar o comando `app deployments <application-name>`, consultando deployments e revisões persistidos no SQLite.

### Resultado esperado

- `app deployments` lista cada deployment com id, commit curto, status e timestamp;
- aplicação sem deployments exibe mensagem informativa;
- aplicação inexistente falha com o erro padrão de aplicação não encontrada;
- resultados ordenados do mais recente para o mais antigo.

### Progresso

- [x] módulo `deployment_list` com `DeploymentSummary` e consulta ao banco;
- [x] comando `app deployments` na CLI;
- [x] testes unitários de lista vazia, múltiplos deployments e isolamento por aplicação;
- [x] testes de CLI para deployment existente, sem histórico e aplicação ausente.

### Critérios de aceite

- [x] aplicação com deployment lista cada tentativa com id, commit curto, status e timestamp;
- [x] aplicação sem deployments exibe mensagem informativa;
- [x] aplicação inexistente falha com o erro padrão;
- [x] resultados ordenados do mais recente para o mais antigo;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- filtros por status ou paginação;
- formato de saída alternativo (JSON, tabular);
- comando `deployment rollback`;
- comando `app expose`;
- TUI.
