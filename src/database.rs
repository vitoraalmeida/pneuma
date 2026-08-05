use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

const INITIAL_MIGRATION_VERSION: i64 = 1;
const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_application_catalog.sql");

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

    let migration_applied = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM schema_migrations WHERE version = ?1
            )",
            [INITIAL_MIGRATION_VERSION],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|source| DatabaseError::Migrate { source })?;

    if migration_applied {
        return Ok(());
    }

    let transaction = connection
        .transaction()
        .map_err(|source| DatabaseError::Migrate { source })?;
    transaction
        .execute_batch(INITIAL_MIGRATION)
        .map_err(|source| DatabaseError::Migrate { source })?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            [INITIAL_MIGRATION_VERSION],
        )
        .map_err(|source| DatabaseError::Migrate { source })?;
    transaction
        .commit()
        .map_err(|source| DatabaseError::Migrate { source })?;

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
        assert_eq!(migration_count, 1);
        assert_eq!(application_table_count, 1);
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
        assert_eq!(migration_count, 1);
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
