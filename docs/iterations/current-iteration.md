# Iteração atual — Health check interno

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** verificar por HTTP se um container candidato em execução está saudável antes de uma futura ativação.

## Trabalho atual — item 22 do roadmap

Adicionar o health check interno mínimo sobre o endpoint loopback observado no Podman:

```text
Container em execução + endpoint loopback
    ↓ GET no caminho configurado
Resposta HTTP esperada
    ↓
Healthy ou causa estruturada de falha
```

### Resultado esperado

- a verificação usa timeout, intervalo e quantidade de tentativas limitados;
- sucesso registra quantidade de tentativas e status HTTP observado;
- falha diferencia timeout, endpoint inalcançável, resposta inválida e status inesperado;
- o checker apenas observa saúde e não promove, para ou remove containers.

### Progresso

- [x] checker HTTP interno síncrono;
- [x] política operacional limitada;
- [x] resultado e causas de falha explícitos;
- [x] testes locais do protocolo e das tentativas;
- [x] teste com container Podman rootless;
- [x] commit `a12c322 feat: check candidate health`.

### Critérios de aceite

- [x] endpoint loopback que responde com o status esperado resulta em `Healthy`;
- [x] status inesperado é retornado como causa estruturada após as tentativas;
- [x] falhas de conexão e timeout não causam panic;
- [x] endpoint que não é loopback é rejeitado antes da conexão;
- [x] uma tentativa posterior pode confirmar saúde durante a inicialização;
- [x] o teste de runtime verifica saúde pelo endpoint real do container;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

O teste end-to-end exige Podman rootless configurado e é executado explicitamente fora da suíte portátil padrão.

## Fora do escopo desta iteração

- persistência do resultado de saúde;
- estados e histórico de deployment;
- promoção ou rejeição automática da candidata;
- health check externo;
- integração com Caddy;
- rollback;
- configuração de timeout e tentativas no manifesto;
- comando de deploy na CLI.
