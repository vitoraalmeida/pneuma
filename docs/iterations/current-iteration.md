# Iteração atual — Progresso detalhado do deployment

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Commits de implementação:** `c11c6f6`, `81aeec4`

**Objetivo:** tornar observável pela CLI o fluxo completo do deployment sem acoplar o Core à apresentação.

## Trabalho atual — item 29 do roadmap

Adicionar o modo global:

```text
pneuma --verbose app deploy <application-name> <repository-path> --revision <revision>
```

### Resultado esperado

- o Core emite eventos estruturados de progresso;
- cada estado persistido é informado depois da transição;
- operações externas longas informam início e conclusão;
- falha e limpeza da candidata também aparecem no fluxo;
- logs detalhados usam `stderr` e o resultado final continua em `stdout`;
- sem `--verbose`, apenas uma mensagem estática informa que o deployment começou.

### Progresso

- [x] tipos concretos de passo e evento de progresso;
- [x] instrumentação da orquestração interna;
- [x] parsing global de `--verbose`;
- [x] apresentação dos eventos pela CLI;
- [x] mensagem estática de início no modo normal;
- [x] testes de saída detalhada e modo padrão.

### Critérios de aceite

- [x] `--verbose` antes do comando ativa o modo detalhado;
- [x] resolução Git, checkout, build e lifecycle da candidata aparecem nos logs;
- [x] estados `Pending` até `Succeeded` aparecem após persistidos;
- [x] health check e promoção aparecem nos logs;
- [x] falha persistida e limpeza aparecem quando aplicáveis;
- [x] logs detalhados vão para `stderr`;
- [x] resultado final permanece em `stdout`;
- [x] execução sem `--verbose` imprime apenas a mensagem estática de início;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- framework ou dependência de logging;
- níveis adicionais como trace, debug ou warn;
- timestamps e medição de duração;
- persistência dos eventos de progresso;
- Caddy e deployment público.
