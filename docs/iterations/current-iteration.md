# Iteração atual — Reconciliação da mesma revisão

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Commit de implementação:** `842397e`

**Objetivo:** tornar repetível o deployment da mesma revisão sem criar deployments ou containers duplicados.

## Trabalho atual — item 30 da sequência de implementação

Antes de criar um deployment, reconciliar a revisão resolvida com o runtime Current já persistido.

### Resultado esperado

- runtime existente e saudável é reutilizado;
- runtime parado ou criado é iniciado novamente e validado;
- runtime ausente é marcado como removido antes de um deployment novo;
- runtime em estado ambíguo ou sem saúde falha sem substituição destrutiva;
- a reconciliação aparece no progresso detalhado.

### Progresso

- [x] consulta do runtime Current para a mesma revisão;
- [x] observação e persistência do estado real;
- [x] reutilização do runtime saudável;
- [x] reinício do runtime parado;
- [x] novo deployment quando o runtime não existe mais;
- [x] testes de integração dos caminhos principais.

### Critérios de aceite

- [x] repetir uma revisão saudável retorna o deployment e runtime existentes;
- [x] a repetição saudável não executa build nem cria outro container;
- [x] runtime parado é reiniciado e passa por health check;
- [x] runtime ausente é marcado como removido e substituído;
- [x] health check sem sucesso não remove nem substitui o Current;
- [x] progresso detalhado informa a reconciliação;
- [x] testes cobrem comportamento observável sem duplicação desnecessária;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- lock entre processos concorrentes;
- substituição automática de runtime existente sem saúde;
- recuperação de deployment interrompido em estado não terminal;
- Caddy e deployment público.
