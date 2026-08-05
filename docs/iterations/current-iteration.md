# Iteração atual — Walking skeleton de importação

**Status:** em andamento

**Atualizado em:** 5 de agosto de 2026

**Objetivo:** importar um repositório local compatível, persistir a aplicação sem duplicidade e permitir sua listagem pela CLI.

## Concluído

### Baseline da aplicação real

- [x] `Containerfile` reproduzível para `vitoralmeida.tech`;
- [x] processo do container executado como usuário sem privilégios;
- [x] porta interna fixa em `8080`;
- [x] `GET /` e `GET /healthz` respondendo HTTP 200;
- [x] `pneuma.toml` criado no repositório do site;
- [x] Podman rootless validado na VPS com o usuário `pneuma`;
- [x] domínio `staging.vitoralmeida.tech` apontando para a VPS;
- [x] Nginx, Caddy e container validados em sequência por HTTP;
- [x] procedimento registrado em [`staging-validation.md`](../operations/staging-validation.md).

HTTPS de staging foi adiado até o ensaio de migração. A exposição HTTP já
comprova o encaminhamento entre as fronteiras atuais.

### Fundação do Pneuma

- [x] pacote Rust mínimo criado;
- [x] Rust 2024 com versão mínima 1.85;
- [x] CI com formatação, Clippy, testes e build release;
- [x] `AGENTS.md` e artefatos de build excluídos do versionamento;
- [x] commit `4115578 chore: initialize Rust workspace`.

### Parser do manifesto — item 16 do roadmap

- [x] carregamento de `pneuma.toml` a partir de um repositório local;
- [x] desserialização tipada do manifesto;
- [x] rejeição de campos desconhecidos;
- [x] validação da versão do schema e dos campos declarados;
- [x] erros de leitura, parsing, incompatibilidade e validação diferenciados;
- [x] oito testes cobrindo sucesso e falhas observáveis;
- [x] checks obrigatórios executados com sucesso;
- [x] commit `0a6dfa6 feat: parse application manifests`.

## Trabalho atual — item 17 do roadmap

Implementar a importação de um repositório local como primeiro fluxo vertical:

```text
CLI
    ↓
ImportApplication
    ↓
Parser do manifesto
    ↓
SQLite
    ↓
ListApplications
```

### Resultado esperado

```bash
pneuma app import /caminho/do/vitoralmeida.tech
pneuma app list
```

A aplicação deve aparecer como registrada e ainda não implantada.

### Progresso

- [x] tipo mínimo de aplicação definido;
- [x] suporte síncrono a SQLite adicionado;
- [x] migration inicial limitada ao catálogo e à especificação do manifesto;
- [x] abertura do banco configura foreign keys e aplica migrations;
- [ ] caso de uso de importação;
- [ ] caso de uso de listagem;
- [ ] comandos de CLI.

### Critérios de aceite

- [ ] um caminho local compatível pode ser importado;
- [ ] manifesto ausente ou inválido produz erro compreensível;
- [x] a migration inicial cria somente as tabelas necessárias para esse fluxo;
- [ ] a aplicação e sua especificação atual são persistidas no SQLite;
- [ ] importar novamente a mesma aplicação não cria duplicidade;
- [ ] `pneuma app list` apresenta a aplicação importada;
- [ ] testes cobrem importação, falha, idempotência e listagem;
- [ ] formatação, Clippy, testes e build release passam sem warnings.

## Fora do escopo desta iteração

- clone remoto;
- resolução de commit;
- build de imagem;
- criação ou supervisão de container;
- health check executado pelo Pneuma;
- alteração de Caddy ou Nginx;
- deployment e rollback;
- TUI.

## Limitações e trabalho adiado

- o milestone v0.0 ainda possui itens operacionais e documentais fora deste fluxo vertical;
- o runtime de staging ainda não é supervisionado por Quadlet/systemd;
- HTTPS de staging ainda não foi configurado;
- a imagem de staging foi transferida manualmente;
- não existe ainda aplicação persistida ou comando CLI funcional.
