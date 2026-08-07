# Iteração atual — Ciclo de vida da aplicação implantada

**Status:** concluída

**Atualizado em:** 7 de agosto de 2026

**Objetivo:** permitir consultar o estado observado e controlar o ciclo de execução (parar e iniciar) de uma aplicação implantada, fechando a jornada do operador da v0.1.

## Trabalho atual — item 36 da sequência de implementação

Adicionar os comandos `app status`, `app stop` e `app start`, persistindo o estado desejado e a última observação do runtime atual.

### Resultado esperado

- deployment promovido marca a aplicação como `running` desejada;
- `app status` observa o container atual no Podman e registra a última observação;
- `app stop` e `app start` atualizam o estado desejado, controlam o container e persistem a observação resultante;
- parar aplicação parada e iniciar aplicação em execução são sucessos idempotentes;
- container removido externamente produz erro claro orientando novo deployment.

### Progresso

- [x] estado desejado atualizado na promoção do deployment;
- [x] comando `app status`;
- [x] comando `app stop`;
- [x] comando `app start`;
- [x] testes de sucesso, idempotência e divergência.

### Critérios de aceite

- [x] aplicação recém-implantada reporta estado desejado `running`;
- [x] status sem deployment informa que a aplicação não está implantada;
- [x] stop persiste `stopped` desejado e observado, e repetir stop é idempotente;
- [x] start persiste `running` desejado e observado, e repetir start é idempotente;
- [x] start/stop/status em aplicação não implantada falham antes de qualquer efeito externo;
- [x] container ausente gera diagnóstico que orienta novo deployment;
- [x] deployment interno e público continuam passando sem alteração de comportamento;
- [x] testes adicionais permanecem proporcionais ao comportamento;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- política de restart e sobrevivência a reboot do host;
- health check no `app start`;
- recuperação automática de container removido externamente;
- TUI;
- comando `app expose`;
- rollback por comando para deployment antigo.
