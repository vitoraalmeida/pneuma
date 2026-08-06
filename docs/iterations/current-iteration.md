# Iteração atual — Orquestração do deployment interno

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** executar uma revisão de aplicação interna do Git até a promoção saudável usando os casos de uso já implementados.

## Trabalho atual — item 27 do roadmap

Compor o primeiro fluxo completo de deployment:

```text
Revisão Git → checkout → build → candidata
    → start/observação → health check → Current
```

### Resultado esperado

- cada efeito externo concluído avança o estado persistido correspondente;
- falhas ficam registradas no estágio em que ocorreram;
- uma candidata criada que falha é removida;
- a runtime `Current` anterior permanece ativa quando a candidata falha;
- aplicações públicas aguardam a integração com Caddy.

### Progresso

- [x] especificação de deployment carregada do catálogo;
- [x] orquestração concreta dos adapters existentes;
- [x] falha estruturada e limpeza da candidata;
- [x] testes de sucesso e falhas representativas;
- [x] commit `907a0cb feat: orchestrate internal deployments`.

### Critérios de aceite

- [x] revisão solicitada é resolvida para um commit completo antes do deployment;
- [x] sucesso percorre os estados até `Succeeded`;
- [x] runtime saudável termina como única `Current` da aplicação;
- [x] falha de build termina em `Failed` no estágio `Building`;
- [x] falha de saúde preserva a `Current` anterior;
- [x] container candidato que falha depois de criado é removido;
- [x] aplicação pública é recusada antes de Git, Podman ou escrita de deployment;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- comando de deploy na CLI;
- Caddy, verificação externa e promoção pública;
- retomada automática após interrupção do processo;
- rollback solicitado pelo usuário;
- política definitiva de retenção de checkouts e imagens.
