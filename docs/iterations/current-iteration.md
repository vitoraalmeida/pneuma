# Iteração atual

**Status:** concluída

**Atualizado em:** 8 de agosto de 2026

## Iteração — Histórico por Release/digest e CLI de visibility (entrega 6 da v0.1 OCI)

Objetivo: alinhar o histórico de deployments ao conceito de Release imutável
(release/digest, não commit_sha) e renomear a CLI de exposição para o termo
"visibility", com saídas coerentes.

### Critérios de aceite

- [x] `app deployments` imprime `DEPLOYMENT | RELEASE | SOURCE | STATUS`, com a
      coluna RELEASE baseada no digest imutável da imagem e SOURCE na revisão de
      origem (`-` para releases entregues por OCI).
- [x] CLI renomeada: `app expose <app> <public|internal>` → `app visibility set
      <app> <public|internal>`; o comando antigo retorna usage.
- [x] Saídas de sucesso e logs verbose alinhadas com o termo "visibility"
      (`Visibility for <app>: Public|Internal`).
- [x] `visibility set` segue operando a materialização do Caddy: público cria o
      fragmento e valida a rota; interno remove o fragmento; o estado desejado é
      persistido separadamente da materialização.
- [x] Testes CLI cobrem o toggle public↔internal, rejeição do comando antigo e
      de visibilidade desconhecida, e a coluna SOURCE (`-`) para OCI.
- [x] Docs (roadmap, scope) refletem a entrega 6.
- [x] Os quatro checks passam: fmt, clippy `-D warnings`, test `--all-features`,
      build `--release`.
