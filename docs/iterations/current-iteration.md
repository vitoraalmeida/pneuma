# Iteração atual — Deployment público utilizável

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Implementação:** `280e442` (`feat: deploy public applications through Caddy`)

**Objetivo:** permitir que `pneuma app deploy` publique uma aplicação no Caddy e só conclua após verificar a rota HTTPS.

## Trabalho atual — item 35 da sequência de implementação

Conectar health interno, aplicação reversível do fragmento Caddy, health externo, promoção da candidata e persistência da exposição.

### Resultado esperado

- deployment interno preserva o comportamento atual;
- deployment público verifica a candidata no endpoint loopback antes de expô-la;
- Caddy recebe a rota candidata por validação e reload seguros;
- a URL HTTPS é verificada pelo domínio via listener local do Caddy;
- somente depois da verificação externa runtime, deployment e exposição são promovidos atomicamente;
- falha externa restaura a rota anterior e remove apenas a candidata.

### Progresso

- [x] rollback explícito de fragmento Caddy aplicado;
- [x] health check HTTPS externo;
- [x] transições públicas do deployment;
- [x] promoção pública transacional;
- [x] persistência de aplicação/falha da exposição;
- [x] configuração CLI dos paths do Caddy;
- [x] testes de sucesso e restauração.

### Critérios de aceite

- [x] aplicação pública deixa de ser recusada pelo comando geral de deployment;
- [x] health interno ocorre antes da troca de tráfego;
- [x] Caddy usa diretório gerenciado e Caddyfile configuráveis;
- [x] health externo usa HTTPS, domínio configurado e path registrado;
- [x] sucesso persiste runtime atual, deployment sucedido e exposição ativa;
- [x] falha do Caddy preserva a rota e runtime anteriores;
- [x] falha do health externo restaura a rota anterior;
- [x] falha pública preserva diagnóstico e remove a candidata;
- [x] deployment interno continua passando sem exigir Caddy ou curl;
- [x] testes adicionais permanecem proporcionais ao comportamento;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- comando separado `app expose`;
- tornar aplicação ativa interna;
- consulta de status e histórico;
- rollback por comando para deployment antigo;
- automação remota após push;
- registry de imagens.
