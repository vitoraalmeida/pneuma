# Iteração atual — Resolução de commit Git

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** resolver uma referência de um repositório Git local para o identificador completo e imutável de um commit.

## Trabalho atual — item 18 do roadmap

Implementar o primeiro incremento do adapter Git previsto para a iteração 2 do plano de entrega:

```text
Repositório local + branch, tag ou SHA
    ↓
Git source control
    ↓
SHA completo do commit
```

### Resultado esperado

- branch, tag anotada ou SHA abreviado resolve para o mesmo commit completo;
- referência inexistente produz erro compreensível;
- objeto Git que não é commit é rejeitado;
- Git é executado com argumentos estruturados, sem shell.

### Progresso

- [x] adapter Git mínimo;
- [x] resolução de branch, tag e SHA;
- [x] erros de execução e resolução diferenciados;
- [x] testes de integração com repositório Git temporário;
- [x] commit `8755870 feat: resolve Git commit references`.

### Critérios de aceite

- [x] uma referência válida resolve para o SHA completo do commit;
- [x] referências inexistentes ou que não apontam para commits são rejeitadas;
- [x] a resolução não altera o repositório;
- [x] testes cobrem sucesso e falhas observáveis sem casos redundantes;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- clone ou fetch remoto;
- checkout ou worktree isolado;
- persistência de revisões;
- comando de deploy na CLI;
- build de imagem;
- criação de container.
