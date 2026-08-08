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
