# Backup e restore do banco

Crie um backup consistente do SQLite com:

```text
pneuma database backup /var/backups/pneuma.sqlite3
```

Restaure um backup validado com:

```text
pneuma database restore /var/backups/pneuma.sqlite3
```

O restore valida `PRAGMA integrity_check`, cria automaticamente uma cópia
`pre-restore` ao lado do banco ativo e substitui o banco de forma atômica. É
uma operação administrativa: não execute outros comandos do Pneuma enquanto
o restore estiver em andamento. Um arquivo de lock impede restores concorrentes.

## Verificação semântica

Um restore correto recupera o estado do instante do backup, não apenas retorna
sucesso. A regressão E2E cria `e2e-before-backup`, gera o backup, cria
`e2e-after-backup` e restaura o arquivo. Depois do restore, o primeiro system
continua presente e o segundo não existe. Esse cenário roda somente em uma VM
descartável, pois o restore substitui o banco ativo.
