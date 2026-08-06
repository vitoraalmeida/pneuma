# Iteração atual — Reativação de runtime Previous

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Commit de implementação:** `1daf8d8`

**Objetivo:** permitir que uma revisão anterior já materializada volte a ser Current sem novo build ou container.

## Trabalho atual — item 31 da sequência de implementação

Estender a reconciliação pré-deployment para runtimes ativos com papel `Previous`.

### Resultado esperado

- runtime Previous existente é observado e reiniciado quando necessário;
- health check ocorre antes de qualquer troca de papel;
- Previous saudável e Current trocam de papel atomicamente;
- falha de observação, start ou saúde preserva os papéis atuais;
- nenhum deployment, build ou container duplicado é criado.

### Progresso

- [x] consulta da revisão entre runtimes Current e Previous;
- [x] reconciliação comum do estado do container;
- [x] troca atômica de papéis após health check;
- [x] progresso detalhado da reativação;
- [x] testes de integração dos caminhos principais.

### Critérios de aceite

- [x] revisão Previous saudável volta a ser Current;
- [x] o antigo Current passa a Previous na mesma transação;
- [x] Previous parado é reiniciado antes da troca;
- [x] Previous sem saúde mantém os papéis originais;
- [x] rollback por revisão não executa build nem cria container;
- [x] saída retorna deployment, runtime e container já existentes;
- [x] testes cobrem comportamento observável sem exagero;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- histórico específico de operações de rollback;
- retenção ou parada automática de runtimes Previous;
- lock entre processos concorrentes;
- Caddy e deployment público.
