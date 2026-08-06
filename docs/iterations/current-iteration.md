# Iteração atual — Persistência da candidata

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** registrar a instância candidata observada em execução e vinculá-la ao deployment, revisão e aplicação corretos.

## Trabalho atual — item 25 do roadmap

Adicionar o registro durável do runtime criado pelo Podman:

```text
Deployment Starting + container Running
    ↓ transação curta
RuntimeInstance Candidate
    ↓
ID externo + endpoint loopback + observação Running
```

### Resultado esperado

- migration 3 cria somente a persistência de instâncias de runtime;
- a instância deriva aplicação e revisão do deployment;
- o registro inicial representa papel `Candidate` e estado observado `Running`;
- o endpoint ativo é IPv4 loopback e exclusivo;
- retry com os mesmos dados é idempotente.

### Progresso

- [x] migration incremental de runtime instances;
- [x] tipo concreto de candidata persistida;
- [x] registro transacional no estado `Starting`;
- [x] idempotência e conflitos explícitos;
- [x] testes de migration, validação e constraints;
- [x] commit `e9ee33a feat: persist candidate runtimes`.

### Critérios de aceite

- [x] banco vazio aplica três migrations em ordem;
- [x] banco na versão 2 recebe runtime instances sem perder deployments;
- [x] candidata persiste aplicação, revisão, deployment e ID externo;
- [x] candidata persiste endpoint loopback, porta interna e observação `Running`;
- [x] deployment fora de `Starting` é rejeitado;
- [x] ID externo, endpoint ou porta inválidos são rejeitados antes da escrita;
- [x] retry idêntico retorna a mesma instância;
- [x] reutilização conflitante de ID externo ou endpoint ativo é rejeitada;
- [x] constraints preservam a associação deployment/aplicação/revisão;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- atualização de observações posteriores;
- promoção para `Current`;
- persistência de resultados de health check;
- orquestração do deployment;
- listagem do histórico;
- Caddy e rollback;
- comando de deploy na CLI.
