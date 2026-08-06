# Iteração atual — Fragmento público gerenciado pelo Caddy

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Commit de implementação:** `16cd34e`

**Objetivo:** materializar com segurança um fragmento Caddy determinístico sem substituir uma rota válida por configuração inválida.

## Trabalho atual — item 32 da sequência de implementação

Criar o primeiro adapter concreto de exposição pública, limitado à geração, validação e substituição atômica do fragmento gerenciado.

### Resultado esperado

- o fragmento usa domínio validado e endpoint exclusivamente loopback;
- cada aplicação escreve somente em `<managed-dir>/<application-id>.caddy`;
- a configuração temporária é validada pelo Caddy antes da ativação;
- a substituição ocorre por rename na mesma filesystem;
- falha de validação preserva o fragmento ativo anterior.

### Progresso

- [x] geração determinística do fragmento;
- [x] validação de identidade, domínio e endpoint;
- [x] arquivo temporário na raiz gerenciada;
- [x] execução de `caddy validate`;
- [x] substituição atômica após sucesso;
- [x] testes de integração com Caddy falso.

### Critérios de aceite

- [x] fragmento válido aponta para o endpoint loopback informado;
- [x] nome do arquivo é derivado apenas de application ID hexadecimal;
- [x] entrada inválida falha antes de escrita ou processo externo;
- [x] validação recebe exatamente o arquivo temporário gerado;
- [x] validação com falha não altera o fragmento anterior;
- [x] sucesso substitui o fragmento e remove o temporário;
- [x] diagnóstico do Caddy é preservado no erro;
- [x] testes cobrem comportamento observável sem exagero;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- reload do Caddy;
- persistência da materialização em SQLite;
- health check externo;
- integração com o fluxo de deployment público;
- remoção de rota para tornar a aplicação interna.
