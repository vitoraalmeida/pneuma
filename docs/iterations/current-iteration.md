# Iteração atual — Lifecycle e observação do runtime

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** iniciar, parar e observar diretamente no Podman o estado de um container criado pelo Pneuma.

## Trabalho atual — item 21 do roadmap

Completar o lifecycle mínimo do adapter de runtime:

```text
Container criado
    ↓ start
Container em execução + endpoint loopback
    ↓ stop
Container parado
    ↓ observe
Estado real consultado no Podman
```

### Resultado esperado

- start e stop preservam a saída do Podman e podem ser repetidos sem criar outra instância;
- status é observado no Podman, não inferido de estado em memória ou banco;
- o endpoint loopback atribuído é retornado enquanto o container está em execução;
- container removido externamente é observado como `Missing`.

### Progresso

- [x] operação de start;
- [x] operação de stop;
- [x] estados observados explícitos;
- [x] descoberta e validação do endpoint loopback;
- [x] teste de lifecycle com Podman rootless;
- [x] commit `a395782 feat: control and observe containers`.

### Critérios de aceite

- [x] container criado pode ser iniciado e observado como `Running`;
- [x] iniciar novamente não cria outra instância nem falha destrutivamente;
- [x] container em execução expõe endpoint em loopback;
- [x] container pode ser parado e observado como `Stopped`;
- [x] parar novamente produz sucesso idempotente ou não destrutivo;
- [x] container removido é observado como `Missing`;
- [x] falhas de controle preservam stdout e stderr para diagnóstico;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

O teste end-to-end exige Podman rootless configurado e é executado explicitamente fora da suíte portátil padrão.

## Fora do escopo desta iteração

- restart como operação dedicada;
- remoção de container pela API do Pneuma;
- supervisão por systemd/Quadlet;
- persistência de runtime ou deployment;
- comando de deploy na CLI;
- health check;
- integração com Caddy;
- rollback.
