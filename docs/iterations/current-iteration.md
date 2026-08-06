# Iteração atual — Transições do deployment

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** avançar um deployment com comparação de estado e registrar falhas terminais estruturadas.

## Trabalho atual — item 24 do roadmap

Implementar a parte da máquina de estados sustentada pelos adapters atuais:

```text
Pending
    ↓ start
PreparingSource
    ↓ source prepared
Building
    ↓ image built
Starting
    ↓ runtime running
VerifyingInternal
    ↘ falha em qualquer etapa
      Failed + causa estruturada
```

### Resultado esperado

- cada avanço compara o estado esperado antes de persistir o próximo;
- retry ou comando fora de ordem retorna conflito sem alterar o deployment;
- o primeiro avanço registra `started_at`;
- falha registra código, etapa, mensagem e `finished_at`;
- nenhum caminho marca o deployment como `Succeeded` antes da futura promoção.

### Progresso

- [x] estados de deployment explícitos;
- [x] eventos de avanço limitados ao fluxo implementado;
- [x] atualização compare-and-set;
- [x] falha terminal estruturada;
- [x] testes da máquina e da persistência;
- [x] commit `b676304 feat: transition pending deployments`.

### Critérios de aceite

- [x] fluxo válido alcança `VerifyingInternal` na ordem definida;
- [x] primeiro avanço preenche `started_at` e avanços seguintes o preservam;
- [x] etapa pulada e retry do mesmo evento produzem conflito observável;
- [x] deployment inexistente produz erro distinto;
- [x] falha persiste estado `Failed`, etapa, código, mensagem e `finished_at`;
- [x] falha libera a exclusão de deployment ativo para uma tentativa posterior;
- [x] estado terminal não volta ao fluxo em andamento;
- [x] não existe transição para `Succeeded` neste incremento;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- orquestração de Git, build, runtime e health check;
- persistência de instâncias de runtime;
- promoção para `Succeeded`;
- estados de troca de tráfego e rollback;
- listagem do histórico;
- Caddy;
- comando de deploy na CLI.
