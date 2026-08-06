# Iteração atual — Persistência da exposição materializada

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Implementação:** `38f6ab6` (`feat: persist exposure materialization state`)

**Objetivo:** distinguir no SQLite a exposição desejada da última rota pública conhecida sem ainda coordenar o deployment com o Caddy.

## Trabalho atual — item 34 da sequência de implementação

Evoluir a tabela `exposures` para registrar runtime ativo, estado de materialização, versão da configuração, horário e último erro.

### Resultado esperado

- bancos existentes são migrados sem perder intenção de exposição;
- exposições existentes começam como `not_materialized`;
- o banco aceita somente estados de materialização conhecidos;
- runtime materializado, quando presente, referencia uma instância persistida;
- importações novas recebem o mesmo estado inicial por default.

### Progresso

- [x] migration incremental de `exposures`;
- [x] estado inicial para registros existentes e novos;
- [x] referência opcional ao runtime ativo;
- [x] campos de versão, horário e diagnóstico;
- [x] testes de banco vazio, upgrade e constraints.

### Critérios de aceite

- [x] migration publicada anteriormente não é alterada;
- [x] banco vazio chega à versão 4;
- [x] upgrade da versão 3 preserva aplicação, exposição e runtime existentes;
- [x] exposição migrada inicia como `not_materialized` sem runtime ou versão ativos;
- [x] estado desconhecido é rejeitado pelo SQLite;
- [x] `active_runtime_id` inexistente é rejeitado pela foreign key;
- [x] migration é idempotente;
- [x] testes adicionais permanecem proporcionais ao comportamento;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- funções de transição da materialização;
- coordenação entre transação SQLite e efeitos no Caddy;
- promoção de deployment público;
- health check externo;
- remoção de rota pública.
