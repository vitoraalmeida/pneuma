# Iteração atual — Criação do runtime

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** criar com Podman rootless um container candidato, parado e identificável, a partir da imagem de uma revisão.

## Trabalho atual — item 20 do roadmap

Implementar o primeiro incremento do adapter de runtime da iteração 2:

```text
Imagem local + aplicação + SHA + porta interna
    ↓
Podman create
    ↓
Container Candidate parado
    ↓
ID externo + publicação loopback configurada
```

### Resultado esperado

- o container recebe nome determinístico e labels de aplicação, revisão e papel;
- a porta interna é configurada somente em `127.0.0.1`; a porta do host será atribuída pelo Podman ao iniciar;
- o container é criado sem modo privilegiado ou mounts do host;
- ID e diagnósticos do Podman ficam acessíveis ao chamador.

### Progresso

- [x] adapter mínimo de criação de container;
- [x] identidade e labels determinísticos;
- [x] publicação de porta limitada ao loopback;
- [x] erros de execução e criação diferenciados;
- [x] teste de integração com Podman rootless;
- [x] commit `cea38ec feat: create candidate containers`.

### Critérios de aceite

- [x] uma imagem local gera um container existente e parado;
- [x] nome, aplicação, revisão e papel podem ser observados no Podman;
- [x] a publicação configurada usa `127.0.0.1` e a porta interna registrada;
- [x] o container não é privilegiado e não possui mounts do host;
- [x] falha de criação preserva stdout e stderr para diagnóstico;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

O teste end-to-end exige Podman rootless configurado e é executado explicitamente fora da suíte portátil padrão.

## Fora do escopo desta iteração

- iniciar, parar, reiniciar ou remover containers pela API do Pneuma;
- supervisão por systemd/Quadlet;
- persistência de runtime ou deployment;
- alocação persistente de portas;
- comando de deploy na CLI;
- health check;
- integração com Caddy;
- rollback.
