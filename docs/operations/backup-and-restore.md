# Database Backup and Restore

Create a consistent SQLite backup with:

```text
pneuma database backup /var/backups/pneuma.sqlite3
```

Restore a validated backup with:

```text
pneuma database restore /var/backups/pneuma.sqlite3
```

Restore validates `PRAGMA integrity_check`, automatically creates a
`pre-restore` copy alongside the active database, and atomically replaces the
database. This is an administrative operation: do not run other Pneuma commands
while restore is in progress. A lock file prevents concurrent restores.

## Semantic Verification

A correct restore recovers the state at the time of the backup, rather than
merely returning success. The E2E regression creates `e2e-before-backup`,
generates the backup, creates `e2e-after-backup`, and restores the file. After
restore, the first system remains present and the second does not exist. This
scenario runs only on a disposable VM because restore replaces the active database.
