# Iteração atual — Comando de deployment interno

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** disponibilizar pela CLI o deployment interno completo já implementado no Core.

## Trabalho atual — item 28 do roadmap

Adicionar o comando estável:

```text
pneuma app deploy <application-name> <repository-path> --revision <revision>
```

### Resultado esperado

- a aplicação importada é localizada pelo nome;
- paths continuam aceitando valores nativos do sistema operacional;
- revisão e nome são validados como texto UTF-8;
- checkouts usam uma raiz gerenciada configurável;
- sucesso imprime commit, deployment, runtime e estado final;
- falhas do caso de uso chegam à fronteira da CLI com status diferente de zero.

### Progresso

- [x] parsing do comando e configuração do workspace;
- [x] resolução da aplicação pelo nome;
- [x] integração com `deploy_internal_revision`;
- [x] testes do comportamento observável da CLI;
- [x] commit `11fe9c6 feat: deploy internal applications from CLI`.

### Critérios de aceite

- [x] sintaxe inválida imprime o uso atualizado e falha;
- [x] aplicação inexistente falha antes de Git ou Podman;
- [x] aplicação interna saudável conclui pela CLI;
- [x] saída de sucesso identifica commit, deployment e runtime;
- [x] `PNEUMA_WORKSPACE_PATH` controla a raiz dos checkouts;
- [x] erro do deployment é preservado e retorna código de falha;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- repositórios remotos sem checkout local fornecido;
- Caddy, verificação externa e deployment público;
- comando de status ou histórico;
- retomada automática após interrupção;
- limpeza definitiva de checkouts e imagens.
