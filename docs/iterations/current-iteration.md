# Iteração atual — Build local por revisão

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Objetivo:** criar um checkout isolado de um commit resolvido e construir uma imagem local determinística com Podman.

## Trabalho atual — item 19 do roadmap

Implementar o incremento de preparação de fonte e build da iteração 2:

```text
Repositório local + SHA completo
    ↓
Checkout isolado
    ↓
Containerfile + contexto registrados
    ↓
Podman build
    ↓
Imagem identificada por aplicação + revisão
```

### Resultado esperado

- duas revisões podem ser preparadas sem alterar uma à outra ou o repositório original;
- `containerfile` e contexto de build permanecem confinados ao checkout;
- a referência da imagem deriva deterministicamente da aplicação e do commit;
- saída e falhas do Podman permanecem disponíveis para diagnóstico.

### Progresso

- [x] criação de checkout Git isolado;
- [x] adapter de build local com Podman;
- [x] confinamento dos paths de build ao checkout;
- [x] identificação determinística da imagem;
- [x] testes de integração de checkout e build;
- [x] commit `742b14d feat: build local images by revision`.

### Critérios de aceite

- [x] checkouts de dois commits preservam conteúdos independentes;
- [x] paths ausentes ou que escapam do checkout são rejeitados antes do build;
- [x] uma imagem local pode ser construída pelo `Containerfile` e contexto registrados;
- [x] a imagem é identificada pela aplicação e pelo SHA completo;
- [x] logs de sucesso e falha do build ficam acessíveis;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

O teste end-to-end com Podman exige um ambiente rootless configurado e é executado explicitamente fora da suíte portátil padrão.

## Fora do escopo desta iteração

- clone ou fetch remoto;
- persistência de revisão ou imagem;
- criação e execução de container;
- comando de deploy na CLI;
- health check;
- integração com Caddy;
- rollback.
