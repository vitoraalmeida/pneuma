# Iteração atual — Reload seguro do Caddy

**Status:** concluída

**Atualizado em:** 6 de agosto de 2026

**Implementação:** `5ed0660` (`feat: reload Caddy with recovery`)

**Objetivo:** aplicar o fragmento gerenciado ao Caddy sem perder a rota anterior quando validação completa ou reload falharem.

## Trabalho atual — item 33 da sequência de implementação

Estender a materialização para validar o Caddyfile principal, executar reload e restaurar atomicamente o fragmento anterior quando necessário.

### Resultado esperado

- o Caddyfile principal é conhecido e validado após instalar o fragmento candidato;
- apenas uma configuração completa válida chega ao reload;
- reload bem-sucedido conclui a materialização;
- falha de validação completa restaura o fragmento anterior sem reload;
- falha de reload restaura o fragmento anterior e recarrega a configuração restaurada;
- erros de recuperação permanecem visíveis junto da falha original.

### Progresso

- [x] configuração do Caddyfile principal;
- [x] captura do fragmento anterior;
- [x] validação da configuração completa;
- [x] reload do Caddy;
- [x] restauração atômica do fragmento;
- [x] reload de recuperação;
- [x] testes com Caddy falso.

### Critérios de aceite

- [x] sucesso valida fragmento e configuração completa antes do reload;
- [x] reload usa o Caddyfile principal informado;
- [x] validação completa com falha restaura o fragmento anterior;
- [x] reload com falha restaura o fragmento e recarrega a versão anterior;
- [x] fragmento inexistente antes da tentativa volta a ficar inexistente na recuperação;
- [x] diagnósticos da falha original e da recuperação são preservados;
- [x] temporários não permanecem após sucesso ou recuperação;
- [x] testes cobrem comportamento observável sem exagero;
- [x] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- persistência da materialização em SQLite;
- health check externo;
- integração com o fluxo de deployment público;
- remoção de rota para tornar a aplicação interna;
- lock global de exposição.
