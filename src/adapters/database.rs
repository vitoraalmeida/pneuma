use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

const INITIAL_MIGRATION: &str = include_str!("../../migrations/0001_application_catalog.sql");
const DEPLOYMENT_MIGRATION: &str =
    include_str!("../../migrations/0002_revisions_and_deployments.sql");
const RUNTIME_MIGRATION: &str = include_str!("../../migrations/0003_runtime_instances.sql");
const EXPOSURE_MIGRATION: &str = include_str!("../../migrations/0004_exposure_materialization.sql");
const SYSTEM_MIGRATION: &str = include_str!("../../migrations/0005_systems.sql");
const RELEASE_MIGRATION: &str = include_str!("../../migrations/0006_releases.sql");
const DEPLOYMENT_RELEASE_MIGRATION: &str =
    include_str!("../../migrations/0007_deployment_release.sql");
const RELEASE_IMAGE_REFERENCE_MIGRATION: &str =
    include_str!("../../migrations/0008_release_image_reference.sql");
const DEPLOYMENT_RELEASE_APPLICATION_MIGRATION: &str =
    include_str!("../../migrations/0009_deployment_release_application.sql");
const RUNTIME_DEPLOYMENT_APPLICATION_MIGRATION: &str =
    include_str!("../../migrations/0010_runtime_deployment_application.sql");
const MIGRATIONS: &[(i64, &str)] = &[
    (1, INITIAL_MIGRATION),
    (2, DEPLOYMENT_MIGRATION),
    (3, RUNTIME_MIGRATION),
    (4, EXPOSURE_MIGRATION),
    (5, SYSTEM_MIGRATION),
    (6, RELEASE_MIGRATION),
    (7, DEPLOYMENT_RELEASE_MIGRATION),
    (8, RELEASE_IMAGE_REFERENCE_MIGRATION),
    (9, DEPLOYMENT_RELEASE_APPLICATION_MIGRATION),
    (10, RUNTIME_DEPLOYMENT_APPLICATION_MIGRATION),
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

        let rebuilds_referenced_tables = version == 7;
        if rebuilds_referenced_tables {
            connection
                .execute_batch("PRAGMA foreign_keys = OFF;")
                .map_err(|source| DatabaseError::Migrate { source })?;
        }

        let migration_result = (|| {
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
                .map_err(|source| DatabaseError::Migrate { source })
        })();

        if rebuilds_referenced_tables {
            connection
                .execute_batch("PRAGMA foreign_keys = ON;")
                .map_err(|source| DatabaseError::Migrate { source })?;
        }
        migration_result?;
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

        assert_eq!(migration_count, 10);
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
        assert_eq!(migration_count, 10);
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
    fn upgrades_runtime_persistence_to_exposure_materialization() {
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
        connection.execute_batch(RUNTIME_MIGRATION).unwrap();
        connection
            .execute_batch(
                "INSERT INTO schema_migrations (version) VALUES (1), (2), (3);
                 INSERT INTO applications (
                    id, name, desired_runtime_state, spec_version, created_at, updated_at
                 ) VALUES ('app-id', 'existing', 'running', 1, 'now', 'now');
                 INSERT INTO exposures (
                    application_id, desired_visibility, domain, created_at, updated_at
                 ) VALUES ('app-id', 'public', 'example.com', 'now', 'now');
                 INSERT INTO revisions (
                    id, application_id, commit_sha, discovered_at
                 ) VALUES (
                    'revision-id', 'app-id',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'now'
                 );
                 INSERT INTO deployments (
                    id, application_id, revision_id, status,
                    requested_at, finished_at, created_at, updated_at
                 ) VALUES (
                    'deployment-id', 'app-id', 'revision-id', 'succeeded',
                    'now', 'now', 'now', 'now'
                 );
                 INSERT INTO runtime_instances (
                    id, application_id, revision_id, deployment_id,
                    external_runtime_id, role, host_address, host_port,
                    container_port, last_observed_state, last_observed_at
                 ) VALUES (
                    'runtime-id', 'app-id', 'revision-id', 'deployment-id',
                    'external-id', 'current', '127.0.0.1', 30001,
                    8080, 'running', 'now'
                 );",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        let exposure: (String, String, Option<String>, String, Option<String>) = connection
            .query_row(
                "SELECT desired_visibility, domain, active_runtime_id,
                        materialization_state, configuration_version
                 FROM exposures WHERE application_id = 'app-id'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        let runtime_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM runtime_instances", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(
            exposure,
            (
                "public".to_owned(),
                "example.com".to_owned(),
                None,
                "not_materialized".to_owned(),
                None,
            )
        );
        assert_eq!(runtime_count, 1);
    }

    #[test]
    fn exposure_materialization_columns_enforce_state_and_runtime_identity() {
        let connection = open(Path::new(":memory:")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO systems (id, name, created_at)
                    VALUES ('system-id', 'test-system', 'now');
                 INSERT INTO applications (
                    id, name, system_id, desired_runtime_state, spec_version, created_at, updated_at
                 ) VALUES ('app-id', 'existing', 'system-id', 'stopped', 1, 'now', 'now');
                 INSERT INTO exposures (
                    application_id, desired_visibility, domain, created_at, updated_at
                 ) VALUES ('app-id', 'public', 'example.com', 'now', 'now');",
            )
            .unwrap();

        let invalid_state = connection
            .execute(
                "UPDATE exposures SET materialization_state = 'unknown'
                 WHERE application_id = 'app-id'",
                [],
            )
            .unwrap_err();
        let missing_runtime = connection
            .execute(
                "UPDATE exposures SET active_runtime_id = 'missing'
                 WHERE application_id = 'app-id'",
                [],
            )
            .unwrap_err();

        assert!(matches!(
            invalid_state,
            rusqlite::Error::SqliteFailure(_, _)
        ));
        assert!(matches!(
            missing_runtime,
            rusqlite::Error::SqliteFailure(_, _)
        ));
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

    #[test]
    fn upgrades_exposure_materialization_to_systems() {
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
        connection.execute_batch(RUNTIME_MIGRATION).unwrap();
        connection.execute_batch(EXPOSURE_MIGRATION).unwrap();
        connection
            .execute_batch(
                "INSERT INTO schema_migrations (version) VALUES (1), (2), (3), (4);
                 INSERT INTO applications (
                    id, name, desired_runtime_state, spec_version, created_at, updated_at
                 ) VALUES ('app-id', 'existing', 'stopped', 1, 'now', 'now');
                 INSERT INTO exposures (
                    application_id, desired_visibility, domain, created_at, updated_at
                 ) VALUES ('app-id', 'public', 'example.com', 'now', 'now');",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        let system_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_schema
                    WHERE type = 'table' AND name = 'systems'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let application_name: String = connection
            .query_row("SELECT name FROM applications", [], |row| row.get(0))
            .unwrap();

        assert!(system_table_exists);
        assert_eq!(application_name, "existing");
    }

    #[test]
    fn upgrades_systems_to_deployment_release() {
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
        connection.execute_batch(RUNTIME_MIGRATION).unwrap();
        connection.execute_batch(EXPOSURE_MIGRATION).unwrap();
        connection.execute_batch(SYSTEM_MIGRATION).unwrap();
        connection.execute_batch(RELEASE_MIGRATION).unwrap();
        connection
            .execute_batch(
                "INSERT INTO schema_migrations (version) VALUES (1), (2), (3), (4), (5), (6);
                 INSERT INTO systems (id, name, created_at)
                    VALUES ('system-id', 'test-system', 'now');
                 INSERT INTO applications (
                    id, name, system_id, desired_runtime_state, spec_version, created_at, updated_at
                 ) VALUES ('app-id', 'existing', 'system-id', 'running', 1, 'now', 'now');
                 INSERT INTO revisions (
                    id, application_id, commit_sha, discovered_at
                 ) VALUES (
                    'revision-id', 'app-id',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'now'
                 );
                 INSERT INTO releases (
                    id, application_id, image_repository, image_digest, source_revision, created_at
                 ) VALUES (
                    'release-id', 'app-id', 'localhost/test',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'now'
                 );
                 INSERT INTO deployments (
                    id, application_id, revision_id, status,
                    requested_at, finished_at, created_at, updated_at
                 ) VALUES (
                    'deployment-id', 'app-id', 'revision-id', 'succeeded',
                    'now', 'now', 'now', 'now'
                 );
                 INSERT INTO runtime_instances (
                    id, application_id, revision_id, deployment_id,
                    external_runtime_id, role, host_address, host_port,
                    container_port, last_observed_state, last_observed_at
                 ) VALUES (
                    'runtime-id', 'app-id', 'revision-id', 'deployment-id',
                    'external-id', 'current', '127.0.0.1', 30001,
                    8080, 'running', 'now'
                 );",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        let release_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_schema
                    WHERE type = 'table' AND name = 'releases'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let deployment_release_id: String = connection
            .query_row("SELECT release_id FROM deployments", [], |row| row.get(0))
            .unwrap();
        let runtime_state: String = connection
            .query_row("SELECT state FROM runtime_instances", [], |row| row.get(0))
            .unwrap();
        let active_deployment_id: Option<String> = connection
            .query_row("SELECT active_deployment_id FROM applications", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert!(release_table_exists);
        assert_eq!(deployment_release_id, "release-id");
        assert_eq!(runtime_state, "running");
        assert_eq!(active_deployment_id, Some("deployment-id".to_owned()));
    }
}
