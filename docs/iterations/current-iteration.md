# Iteração atual — Promoção interna da candidata

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** promover uma candidata interna somente depois de confirmar sua saúde no endpoint loopback.

## Trabalho atual — item 26 do roadmap

Adicionar a primeira conclusão segura de deployment para aplicações internas:

```text
Candidate Running + Deployment VerifyingInternal
    ↓ health check interno
Healthy
    ↓ transação curta
Current anterior → Previous
Candidate → Current
Deployment → Succeeded
```

### Resultado esperado

- o health check ocorre fora da transação de promoção;
- estado, papel e visibilidade são revalidados antes da escrita;
- somente aplicações internas podem usar este caminho;
- uma candidata não saudável leva o deployment a `Failed` sem substituir a atual;
- retry de uma promoção concluída é idempotente.

### Progresso

- [x] estado `Succeeded` representado no fluxo atual;
- [x] verificação interna vinculada ao endpoint persistido;
- [x] promoção transacional de papéis;
- [x] falha de saúde registrada no deployment;
- [x] testes de primeira promoção, substituição e falha;
- [x] commit `7fcb601 feat: promote healthy internal candidates`.

### Critérios de aceite

- [x] candidata interna saudável torna-se `Current`;
- [x] deployment promovido torna-se `Succeeded` com `finished_at`;
- [x] `Current` anterior torna-se `Previous` na mesma transação;
- [x] nunca existem duas runtimes `Current` para a aplicação;
- [x] health check usa o endpoint persistido da candidata;
- [x] candidata não saudável deixa o deployment `Failed` com causa estruturada;
- [x] falha da candidata preserva a runtime `Current` anterior;
- [x] aplicação pública é recusada antes do health check;
- [x] retry da mesma promoção retorna o resultado persistido;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- orquestração de Git, build e criação da candidata;
- persistência detalhada do resultado do health check;
- atualização de observações posteriores;
- promoção de aplicações públicas;
- Caddy, verificação externa e rollback;
- comando de deploy na CLI.
