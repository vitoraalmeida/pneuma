# Iteração atual — Início persistido de deployment

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** registrar de forma atômica uma revisão imutável e uma nova tentativa de deployment em estado `Pending`.

## Trabalho atual — item 23 do roadmap

Adicionar o ponto inicial durável do processo de deployment:

```text
Aplicação importada + commit resolvido
    ↓ transação curta
Revision única por aplicação e commit
    ↓
Deployment Pending
```

### Resultado esperado

- migrations criam somente as tabelas `revisions` e `deployments` deste incremento;
- a revisão pertence à aplicação e pode ser reutilizada por tentativas futuras;
- todo deployment nasce em estado explícito `Pending` com horário de solicitação;
- somente um deployment não terminal pode existir por aplicação.

### Progresso

- [x] migration incremental de revisões e deployments;
- [x] tipos concretos de revisão e deployment pendente;
- [x] criação transacional da tentativa;
- [x] exclusão de tentativas simultâneas por aplicação;
- [x] testes de migration e persistência;
- [x] commit `8b3a62c feat: persist pending deployments`.

### Critérios de aceite

- [x] banco vazio aplica as migrations 1 e 2 em ordem;
- [x] banco na versão 1 recebe a versão 2 sem perder o catálogo;
- [x] commit completo cria ou reutiliza uma revisão da aplicação;
- [x] nova tentativa é persistida como `Pending` com timestamps;
- [x] aplicação inexistente e commit inválido produzem erros estruturados;
- [x] segunda tentativa não terminal para a mesma aplicação é rejeitada;
- [x] constraints impedem associar deployment à revisão de outra aplicação;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- transições posteriores a `Pending`;
- execução coordenada de Git, build, runtime e health check;
- persistência de instâncias de runtime;
- persistência de resultados de health check;
- listagem do histórico;
- promoção, Caddy e rollback;
- comando de deploy na CLI.
