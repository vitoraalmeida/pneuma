use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, DatabaseName, OpenFlags, Transaction};
use thiserror::Error;

pub(crate) const DATABASE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_DATABASE_PATH";
pub(crate) const DEFAULT_DATABASE_PATH: &str = "/var/lib/pneuma/database/pneuma.sqlite3";

// The one current baseline migration and its textual ledger identity. The
// identity cannot be confused with the retired integer-only ledger.
const BASELINE_MIGRATION_ID: &str = "0001_current_schema";
const BASELINE_MIGRATION: &str = include_str!("../../migrations/0001_current_schema.sql");

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("failed to open database at {}: {source}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to configure database: {source}")]
    Configure {
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to initialize database schema: {source}")]
    Initialize {
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "database at {} uses an incompatible schema; only the current Pneuma schema is supported and old databases are not upgraded",
        path.display()
    )]
    IncompatibleSchema { path: PathBuf },
    #[error("backup destination already exists: {}", path.display())]
    BackupDestinationExists { path: PathBuf },
    #[error(
        "failed to create backup directory {}: {source}",
        path.display()
    )]
    BackupDestinationParent {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("database backup failed: {source}")]
    Backup {
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to open restore source {}: {source}", path.display())]
    RestoreSource {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "restore source {} failed integrity check: {result}",
        path.display()
    )]
    RestoreIntegrity { path: PathBuf, result: String },
    #[error(
        "another Pneuma command is using the database: {}",
        path.display()
    )]
    DatabaseBusy { path: PathBuf },
    #[error(
        "failed to acquire the database-wide lock {}: {source}",
        path.display()
    )]
    DatabaseLock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to replace database during restore: {source}")]
    RestoreReplace {
        #[source]
        source: io::Error,
    },
}

// Access mode for the database-wide kernel lock. Normal commands share the
// database; restore must observe no concurrent access at all.
pub enum LockMode {
    Shared,
    Exclusive,
}

// Holds the database-wide kernel lock until dropped; process death releases it.
pub struct DatabaseLock {
    file: File,
}

impl DatabaseLock {
    // Tries to acquire the database-wide lock without blocking. `None` means
    // another holder conflicts with the requested mode.
    pub fn try_acquire(
        database_path: &Path,
        mode: LockMode,
    ) -> Result<Option<Self>, DatabaseError> {
        let path = lock_path(database_path);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| DatabaseError::DatabaseLock {
                path: path.clone(),
                source,
            })?;
        // flock is process-scoped and releases automatically if the process exits.
        let operation = match mode {
            LockMode::Shared => libc::LOCK_SH,
            LockMode::Exclusive => libc::LOCK_EX,
        };
        let result = unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) };
        if result == 0 {
            return Ok(Some(Self { file }));
        }
        let source = std::io::Error::last_os_error();
        if source.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        Err(DatabaseError::DatabaseLock { path, source })
    }
}

impl Drop for DatabaseLock {
    fn drop(&mut self) {
        // Keep the sidecar inode stable: unlinking it could let a later process lock a new file.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

// Names the database-wide lock sidecar from the configured database identity.
fn lock_path(database_path: &Path) -> PathBuf {
    if database_path.as_os_str().is_empty() || database_path == Path::new(":memory:") {
        return std::env::temp_dir().join("pneuma-memory-database.lock");
    }
    PathBuf::from(format!("{}.lock", database_path.display()))
}

// Creates a SQLite-consistent backup without overwriting an existing operator-selected destination.
pub fn backup(path: &Path, destination: &Path) -> Result<(), DatabaseError> {
    if destination.exists() {
        return Err(DatabaseError::BackupDestinationExists {
            path: destination.to_path_buf(),
        });
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source| DatabaseError::BackupDestinationParent {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|source| {
            DatabaseError::Open {
                path: path.to_path_buf(),
                source,
            }
        })?;
    connection
        .backup(DatabaseName::Main, destination, None)
        .map_err(|source| DatabaseError::Backup { source })
}

// Validates a backup against the current schema, serializes restoration against
// every other database user, and returns the automatically created pre-restore backup.
pub fn restore_and_verify(path: &Path, source_path: &Path) -> Result<PathBuf, DatabaseError> {
    let _lock = DatabaseLock::try_acquire(path, LockMode::Exclusive)?.ok_or_else(|| {
        DatabaseError::DatabaseBusy {
            path: path.to_path_buf(),
        }
    })?;
    let pre_restore = restore(path, source_path)?;
    let _ = open(path)?;
    Ok(pre_restore)
}

// Resolves the configured database path, treating an empty override as unset.
pub fn configured_path() -> PathBuf {
    crate::config::configured_path(DATABASE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_DATABASE_PATH)
}

// Returns the migration ledger identity recorded by an already-open database.
pub fn migration_identity(connection: &Connection) -> Result<String, rusqlite::Error> {
    connection.query_row("SELECT migration_id FROM schema_migrations", [], |row| {
        row.get(0)
    })
}

// Validates the restore source, then replaces the database under the caller's exclusive lock.
fn restore(path: &Path, source_path: &Path) -> Result<PathBuf, DatabaseError> {
    let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|source| DatabaseError::RestoreSource {
            path: source_path.to_path_buf(),
            source,
        })?;
    let integrity: String = source
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|source| DatabaseError::RestoreSource {
            path: source_path.to_path_buf(),
            source,
        })?;
    if integrity != "ok" {
        return Err(DatabaseError::RestoreIntegrity {
            path: source_path.to_path_buf(),
            result: integrity,
        });
    }
    // The backup must already carry the exact current schema: an old-generation
    // or malformed backup is rejected before the live database is touched.
    verify_current_ledger(&source, source_path)?;
    restore_locked(path, &source)
}

// Replaces the database while the caller holds the exclusive database-wide lock.
fn restore_locked(path: &Path, source: &Connection) -> Result<PathBuf, DatabaseError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pre_restore = path.with_extension(format!("pre-restore-{timestamp}.sqlite3"));
    backup(path, &pre_restore)?;
    let temporary = path.with_extension("restore.tmp");
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|source| DatabaseError::RestoreReplace { source })?;
    }
    source
        .backup(DatabaseName::Main, &temporary, None)
        .map_err(|source| DatabaseError::Backup { source })?;
    fs::rename(&temporary, path).map_err(|source| DatabaseError::RestoreReplace { source })?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = PathBuf::from(format!("{}{}", path.display(), suffix));
        if sidecar.exists() {
            fs::remove_file(sidecar).map_err(|source| DatabaseError::RestoreReplace { source })?;
        }
    }
    Ok(pre_restore)
}

// Opens a connection with foreign keys enforced, initializing the current
// schema on an empty database and rejecting every other non-current schema.
pub fn open(path: &Path) -> Result<Connection, DatabaseError> {
    let mut connection = Connection::open(path).map_err(|source| DatabaseError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|source| DatabaseError::Configure { source })?;
    initialize_or_verify(&mut connection, path)?;

    Ok(connection)
}

// Initializes an empty database atomically or verifies an existing one carries
// exactly the current migration ledger; anything else is incompatible.
fn initialize_or_verify(connection: &mut Connection, path: &Path) -> Result<(), DatabaseError> {
    let user_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|source| DatabaseError::Configure { source })?;

    if user_tables == 0 {
        let transaction = connection
            .transaction()
            .map_err(|source| DatabaseError::Initialize { source })?;
        apply_baseline(&transaction, BASELINE_MIGRATION, BASELINE_MIGRATION_ID)?;
        transaction
            .commit()
            .map_err(|source| DatabaseError::Initialize { source })?;
        return Ok(());
    }

    verify_current_ledger(connection, path)
}

// Applies one baseline migration and records its ledger identity in the same transaction.
fn apply_baseline(
    transaction: &Transaction<'_>,
    sql: &str,
    migration_id: &str,
) -> Result<(), DatabaseError> {
    transaction
        .execute_batch(sql)
        .map_err(|source| DatabaseError::Initialize { source })?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (migration_id) VALUES (?1)",
            [migration_id],
        )
        .map_err(|source| DatabaseError::Initialize { source })?;
    Ok(())
}

// Accepts only the exact current ledger: one row carrying the baseline identity.
fn verify_current_ledger(connection: &Connection, path: &Path) -> Result<(), DatabaseError> {
    let incompatible = || DatabaseError::IncompatibleSchema {
        path: path.to_path_buf(),
    };
    let mut statement = connection
        .prepare("SELECT migration_id FROM schema_migrations")
        .map_err(|_| incompatible())?;
    let identities = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| incompatible())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| incompatible())?;
    match identities.as_slice() {
        [id] if id == BASELINE_MIGRATION_ID => Ok(()),
        _ => Err(incompatible()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::process::{Command, Stdio};

    use super::*;

    const EXPECTED_TABLES: [&str; 8] = [
        "schema_migrations",
        "systems",
        "applications",
        "releases",
        "deployments",
        "runtime_instances",
        "exposures",
        "runtime_port_reservations",
    ];

    #[test]
    fn open_initializes_the_exact_current_schema_with_foreign_keys() {
        let connection = open(Path::new(":memory:")).unwrap();

        let foreign_keys: bool = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert!(foreign_keys);

        let mut tables: Vec<String> = connection
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        tables.sort();
        let mut expected = EXPECTED_TABLES
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            tables, expected,
            "the schema must contain exactly the eight current tables"
        );

        let ledger = current_ledger(&connection).unwrap();
        assert_eq!(ledger, vec![BASELINE_MIGRATION_ID.to_owned()]);
    }

    #[test]
    fn reopening_a_current_database_is_idempotent() {
        let path = std::env::temp_dir().join(format!(
            "pneuma-reopen-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_file(&path);

        {
            let connection = open(&path).unwrap();
            connection
                .execute("INSERT INTO systems (id, name) VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'team')", [])
                .unwrap();
        }
        {
            let connection = open(&path).unwrap();
            let count: i64 = connection
                .query_row("SELECT COUNT(*) FROM systems", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
            let ledger = current_ledger(&connection).unwrap();
            assert_eq!(ledger, vec![BASELINE_MIGRATION_ID.to_owned()]);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_nonempty_database_without_the_current_ledger_is_rejected() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
            .unwrap();

        let error = initialize_or_verify(&mut connection, Path::new(":memory:")).unwrap_err();
        assert!(matches!(error, DatabaseError::IncompatibleSchema { .. }));

        // The same rejection applies through open() for a file database.
        let path = std::env::temp_dir().join(format!(
            "pneuma-incompatible-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_file(&path);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
            .unwrap();
        drop(connection);

        let error = open(&path).unwrap_err();
        assert!(matches!(error, DatabaseError::IncompatibleSchema { .. }));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_retired_integer_ledger_is_rejected_as_incompatible() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (
                     version INTEGER PRIMARY KEY,
                     applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );
                 INSERT INTO schema_migrations (version) VALUES (14);",
            )
            .unwrap();

        let error = verify_current_ledger(&connection, Path::new(":memory:")).unwrap_err();
        assert!(matches!(error, DatabaseError::IncompatibleSchema { .. }));
    }

    #[test]
    fn a_failed_baseline_application_persists_nothing() {
        let mut connection = Connection::open_in_memory().unwrap();
        let transaction = connection.transaction().unwrap();
        let broken = format!("{BASELINE_MIGRATION}\nTHIS IS NOT VALID SQL;");
        let error = apply_baseline(&transaction, &broken, BASELINE_MIGRATION_ID);
        assert!(error.is_err(), "invalid baseline SQL must fail");
        drop(transaction);

        let tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "a failed baseline must leave the database empty");
    }

    #[test]
    fn public_domains_are_owned_case_insensitively_by_one_application() {
        let connection = open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "first");
        seed_application(&connection, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "second");

        connection
            .execute(
                "INSERT INTO exposures (application_id, desired_visibility, domain, materialization_state)
                 VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'public', 'Example.COM', 'not_materialized')",
                [],
            )
            .unwrap();
        let error = connection
            .execute(
                "INSERT INTO exposures (application_id, desired_visibility, domain, materialization_state)
                 VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'public', 'example.com', 'not_materialized')",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    #[test]
    fn exposure_route_and_diagnostic_evidence_is_all_or_nothing() {
        let connection = open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "app");

        let error = connection
            .execute(
                "INSERT INTO exposures (
                     application_id, desired_visibility, domain,
                     materialization_state, configuration_version, last_materialized_at
                 ) VALUES (
                     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'internal', NULL,
                     'active', 'route bytes', '2026-01-01')",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        ));

        let error = connection
            .execute(
                "INSERT INTO exposures (
                     application_id, desired_visibility, domain,
                     materialization_state, last_error_code
                 ) VALUES (
                     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'internal', NULL,
                     'failed', 'caddy_materialization_failed')",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    #[test]
    fn runtime_ownership_and_retirement_constraints_are_enforced() {
        let connection = open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "app");
        connection
            .execute_batch(
                "INSERT INTO releases (id, application_id, image_reference, created_at)
                 VALUES ('cccccccccccccccccccccccccccccccc', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                         'registry.example/app@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
                         'now');
                 INSERT INTO deployments (id, application_id, release_id, type, status, requested_at)
                 VALUES ('dddddddddddddddddddddddddddddddd', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                         'cccccccccccccccccccccccccccccccc', 'deploy', 'pending', 'now');",
            )
            .unwrap();

        // A runtime must belong to the Application that owns its Deployment.
        connection
            .execute(
                "INSERT INTO runtime_instances (
                     id, application_id, deployment_id, external_runtime_id, state,
                     host_port, container_port, last_observed_state, last_observed_at
                 ) VALUES ('eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                           'dddddddddddddddddddddddddddddddd', 'aabbccdd', 'running',
                           30000, 8080, 'running', 'now')",
                [],
            )
            .unwrap();
        let error = connection
            .execute(
                "INSERT INTO runtime_instances (
                     id, application_id, deployment_id, external_runtime_id, state,
                     host_port, container_port, last_observed_state, last_observed_at
                 ) VALUES ('ffffffffffffffffffffffffffffffff', 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                           'dddddddddddddddddddddddddddddddd', 'ddeeff00', 'running',
                           30001, 8080, 'running', 'now')",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        ));

        // A live running runtime cannot carry retirement evidence.
        let error = connection
            .execute(
                "UPDATE runtime_instances SET removed_at = '2026-01-01'
                 WHERE id = 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    #[test]
    fn one_deployment_holds_at_most_one_port_reservation() {
        let connection = open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "app");
        connection
            .execute_batch(
                "INSERT INTO releases (id, application_id, image_reference, created_at)
                 VALUES ('cccccccccccccccccccccccccccccccc', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                         'registry.example/app@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
                         'now');
                 INSERT INTO deployments (id, application_id, release_id, type, status, requested_at)
                 VALUES ('dddddddddddddddddddddddddddddddd', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                         'cccccccccccccccccccccccccccccccc', 'deploy', 'pending', 'now');",
            )
            .unwrap();

        connection
            .execute(
                "INSERT INTO runtime_port_reservations (port, application_id, deployment_id)
                 VALUES (30000, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dddddddddddddddddddddddddddddddd')",
                [],
            )
            .unwrap();
        let error = connection
            .execute(
                "INSERT INTO runtime_port_reservations (port, application_id, deployment_id)
                 VALUES (30001, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dddddddddddddddddddddddddddddddd')",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    #[test]
    fn database_lock_shared_and_exclusive_modes_serialize_access() {
        let root = temporary_test_directory("database-lock-modes");
        let database = root.join("pneuma.sqlite3");

        let shared_a = DatabaseLock::try_acquire(&database, LockMode::Shared)
            .unwrap()
            .unwrap();
        let shared_b = DatabaseLock::try_acquire(&database, LockMode::Shared)
            .unwrap()
            .unwrap();
        assert!(
            DatabaseLock::try_acquire(&database, LockMode::Exclusive)
                .unwrap()
                .is_none(),
            "exclusive restore must wait while normal access holds the lock"
        );
        drop(shared_a);
        drop(shared_b);

        let exclusive = DatabaseLock::try_acquire(&database, LockMode::Exclusive)
            .unwrap()
            .unwrap();
        assert!(
            DatabaseLock::try_acquire(&database, LockMode::Shared)
                .unwrap()
                .is_none(),
            "normal access must wait while restore holds the lock"
        );
        assert!(
            DatabaseLock::try_acquire(&database, LockMode::Exclusive)
                .unwrap()
                .is_none()
        );
        drop(exclusive);

        assert!(
            DatabaseLock::try_acquire(&database, LockMode::Shared)
                .unwrap()
                .is_some()
        );
        fs::remove_file(lock_path(&database)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn releases_the_database_lock_when_the_holding_process_dies() {
        let root = temporary_test_directory("database-lock-process");
        let database = root.join("pneuma.sqlite3");
        let path = lock_path(&database);
        let script = format!(
            "exec 9>\"{}\"; flock -n 9; printf ready; while :; do :; done",
            path.display()
        );
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = [0; 5];
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        assert_eq!(&ready, b"ready");
        assert!(
            DatabaseLock::try_acquire(&database, LockMode::Exclusive)
                .unwrap()
                .is_none()
        );

        child.kill().unwrap();
        child.wait().unwrap();

        assert!(
            DatabaseLock::try_acquire(&database, LockMode::Exclusive)
                .unwrap()
                .is_some()
        );
        fs::remove_file(path).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_replaces_the_live_database_from_an_exact_current_backup() {
        let root = temporary_test_directory("restore-current");
        let live = root.join("pneuma.sqlite3");
        {
            let connection = open(&live).unwrap();
            connection
                .execute(
                    "INSERT INTO systems (id, name) VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'team')",
                    [],
                )
                .unwrap();
        }
        let backup_path = root.join("backup.sqlite3");
        backup(&live, &backup_path).unwrap();
        {
            let connection = open(&live).unwrap();
            connection.execute("DELETE FROM systems", []).unwrap();
        }

        let pre_restore = restore_and_verify(&live, &backup_path).unwrap();

        assert!(pre_restore.exists(), "the pre-restore snapshot must exist");
        let connection = open(&live).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM systems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "the restored catalog must carry the backup state");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_rejects_an_incompatible_backup_before_touching_the_live_database() {
        let root = temporary_test_directory("restore-incompatible");
        let live = root.join("pneuma.sqlite3");
        {
            let connection = open(&live).unwrap();
            connection
                .execute(
                    "INSERT INTO systems (id, name) VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'team')",
                    [],
                )
                .unwrap();
        }
        let old_backup = root.join("old-generation.sqlite3");
        let connection = Connection::open(&old_backup).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (
                     version INTEGER PRIMARY KEY,
                     applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );
                 INSERT INTO schema_migrations (version) VALUES (14);",
            )
            .unwrap();
        drop(connection);

        let error = restore_and_verify(&live, &old_backup).unwrap_err();

        assert!(matches!(error, DatabaseError::IncompatibleSchema { .. }));
        let connection = open(&live).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM systems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "the live database must stay untouched");
        assert!(
            !contains_pre_restore_snapshot(&root),
            "no pre-restore snapshot may be created for a rejected backup"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_rejects_a_corrupt_backup_before_touching_the_live_database() {
        let root = temporary_test_directory("restore-corrupt");
        let live = root.join("pneuma.sqlite3");
        {
            let connection = open(&live).unwrap();
            connection
                .execute(
                    "INSERT INTO systems (id, name) VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'team')",
                    [],
                )
                .unwrap();
        }
        let corrupt = root.join("corrupt.sqlite3");
        fs::write(&corrupt, b"this is not a sqlite database").unwrap();

        let error = restore_and_verify(&live, &corrupt).unwrap_err();

        assert!(matches!(
            error,
            DatabaseError::RestoreSource { .. } | DatabaseError::RestoreIntegrity { .. }
        ));
        let connection = open(&live).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM systems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "the live database must stay untouched");
        assert!(
            !contains_pre_restore_snapshot(&root),
            "no pre-restore snapshot may be created for a rejected backup"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_reports_busy_while_another_holder_conflicts_with_the_exclusive_lock() {
        let root = temporary_test_directory("restore-busy");
        let live = root.join("pneuma.sqlite3");
        {
            let connection = open(&live).unwrap();
            connection
                .execute(
                    "INSERT INTO systems (id, name) VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'team')",
                    [],
                )
                .unwrap();
        }
        let backup_path = root.join("backup.sqlite3");
        backup(&live, &backup_path).unwrap();
        let _shared = DatabaseLock::try_acquire(&live, LockMode::Shared)
            .unwrap()
            .unwrap();

        let error = restore_and_verify(&live, &backup_path).unwrap_err();

        assert!(matches!(error, DatabaseError::DatabaseBusy { .. }));
        let connection = open(&live).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM systems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "a busy restore must not replace the database");
        fs::remove_dir_all(root).unwrap();
    }

    fn contains_pre_restore_snapshot(root: &Path) -> bool {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .any(|name| name.contains("pre-restore"))
    }

    fn temporary_test_directory(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pneuma-database-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn current_ledger(connection: &Connection) -> Result<Vec<String>, rusqlite::Error> {
        let mut statement = connection.prepare("SELECT migration_id FROM schema_migrations")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    fn seed_application(connection: &Connection, id: &str, name: &str) {
        connection
            .execute_batch(&format!(
                "INSERT INTO systems (id, name) VALUES ('{id}', 'team-{name}');
                 INSERT INTO applications (
                     id, system_id, name, repository_url, default_branch, manifest_path,
                     image_repository, container_port, health_check_path,
                     health_check_expected_status, desired_runtime_state
                 ) VALUES (
                     '{id}', '{id}', '{name}', 'https://example.test/app.git', 'main',
                     'pneuma.toml', 'registry.example/app', 8080, '/healthz', 200, 'stopped');"
            ))
            .unwrap();
    }
}
