use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_application_catalog.sql");
const DEPLOYMENT_MIGRATION: &str = include_str!("../migrations/0002_revisions_and_deployments.sql");
const RUNTIME_MIGRATION: &str = include_str!("../migrations/0003_runtime_instances.sql");
const MIGRATIONS: &[(i64, &str)] = &[
    (1, INITIAL_MIGRATION),
    (2, DEPLOYMENT_MIGRATION),
    (3, RUNTIME_MIGRATION),
];

#[derive(Debug)]
pub enum DatabaseError {
    Open {
        path: PathBuf,
        source: rusqlite::Error,
    },
    Configure {
        source: rusqlite::Error,
    },
    Migrate {
        source: rusqlite::Error,
    },
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { path, source } => {
                write!(
                    formatter,
                    "failed to open database at {}: {source}",
                    path.display()
                )
            }
            Self::Configure { source } => {
                write!(formatter, "failed to configure database: {source}")
            }
            Self::Migrate { source } => write!(formatter, "failed to migrate database: {source}"),
        }
    }
}

impl Error for DatabaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } | Self::Configure { source } | Self::Migrate { source } => {
                Some(source)
            }
        }
    }
}

pub fn open(path: &Path) -> Result<Connection, DatabaseError> {
    let mut connection = Connection::open(path).map_err(|source| DatabaseError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|source| DatabaseError::Configure { source })?;
    migrate(&mut connection)?;

    Ok(connection)
}

fn migrate(connection: &mut Connection) -> Result<(), DatabaseError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .map_err(|source| DatabaseError::Migrate { source })?;

    for &(version, sql) in MIGRATIONS {
        let migration_applied = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM schema_migrations WHERE version = ?1
                )",
                [version],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|source| DatabaseError::Migrate { source })?;
        if migration_applied {
            continue;
        }

        let transaction = connection
            .transaction()
            .map_err(|source| DatabaseError::Migrate { source })?;
        transaction
            .execute_batch(sql)
            .map_err(|source| DatabaseError::Migrate { source })?;
        transaction
            .execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [version],
            )
            .map_err(|source| DatabaseError::Migrate { source })?;
        transaction
            .commit()
            .map_err(|source| DatabaseError::Migrate { source })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_configures_and_migrates_database() {
        let connection = open(Path::new(":memory:")).unwrap();

        let foreign_keys: bool = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        let migration_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let application_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'applications'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(foreign_keys);
        let deployment_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'deployments'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(migration_count, 3);
        assert_eq!(application_table_count, 1);
        assert_eq!(deployment_table_count, 1);
    }

    #[test]
    fn migration_is_idempotent() {
        let mut connection = open(Path::new(":memory:")).unwrap();

        migrate(&mut connection).unwrap();

        let migration_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(migration_count, 3);
    }

    #[test]
    fn upgrades_an_existing_catalog_to_deployment_persistence() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );",
            )
            .unwrap();
        connection.execute_batch(INITIAL_MIGRATION).unwrap();
        connection
            .execute("INSERT INTO schema_migrations (version) VALUES (1)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO applications (
                    id, name, desired_runtime_state, spec_version, created_at, updated_at
                 ) VALUES ('app-id', 'existing', 'stopped', 1, 'now', 'now')",
                [],
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        let application_name: String = connection
            .query_row("SELECT name FROM applications", [], |row| row.get(0))
            .unwrap();
        let deployment_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_schema
                    WHERE type = 'table' AND name = 'deployments'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(application_name, "existing");
        assert!(deployment_table_exists);
    }

    #[test]
    fn upgrades_deployment_persistence_to_runtime_instances() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );",
            )
            .unwrap();
        connection.execute_batch(INITIAL_MIGRATION).unwrap();
        connection.execute_batch(DEPLOYMENT_MIGRATION).unwrap();
        connection
            .execute_batch(
                "INSERT INTO schema_migrations (version) VALUES (1), (2);
                 INSERT INTO applications (
                    id, name, desired_runtime_state, spec_version, created_at, updated_at
                 ) VALUES ('app-id', 'existing', 'stopped', 1, 'now', 'now');
                 INSERT INTO revisions (
                    id, application_id, commit_sha, discovered_at
                 ) VALUES (
                    'revision-id', 'app-id',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'now'
                 );
                 INSERT INTO deployments (
                    id, application_id, revision_id, status,
                    requested_at, created_at, updated_at
                 ) VALUES (
                    'deployment-id', 'app-id', 'revision-id', 'starting',
                    'now', 'now', 'now'
                 );",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        let deployment_status: String = connection
            .query_row("SELECT status FROM deployments", [], |row| row.get(0))
            .unwrap();
        let runtime_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_schema
                    WHERE type = 'table' AND name = 'runtime_instances'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(deployment_status, "starting");
        assert!(runtime_table_exists);
    }

    #[test]
    fn migration_enforces_application_source_relationship() {
        let connection = open(Path::new(":memory:")).unwrap();

        let error = connection
            .execute(
                "INSERT INTO application_sources (
                    application_id,
                    repository_location,
                    repository_kind,
                    default_branch,
                    manifest_path,
                    created_at,
                    updated_at
                ) VALUES ('missing', '.', 'local', 'main', 'pneuma.toml', 'now', 'now')",
                [],
            )
            .unwrap_err();

        assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
    }
}
