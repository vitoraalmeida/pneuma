use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use thiserror::Error;

use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::persistence::{invalid_text_value, outcome, visibility_from_value};
use crate::domain::application::{
    Application, ApplicationDeploymentSpecification, ApplicationName, ApplicationSummary,
    DesiredRuntimeState,
};
use crate::domain::git::{ApplicationSource, RelativeManifestPath, RepositoryKind};
use crate::domain::identity::{ApplicationId, DeploymentId, SystemId};
use crate::domain::release::DeliverySpecification;
use crate::domain::release::DeliveryType;
use crate::domain::release::OciRepository;
use crate::domain::runtime::{
    ContainerPort, HealthCheckPath, HealthCheckSpecification, HealthCheckStatus,
    RuntimeSpecification,
};

// Every lookup in this store is an optional query: absence is `Ok(None)`, never
// a dedicated error variant. Callers that require a row translate `None` into
// their own domain error.
#[derive(Debug, Error)]
pub enum ApplicationStoreError {
    #[error("application `{application_id}` has invalid desired runtime state `{state}`")]
    InvalidDesiredRuntimeState {
        application_id: String,
        state: String,
    },
    #[error("application store error: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

// Allocates an ID inside the import transaction so related Application records share one boundary.
pub(crate) fn generate_id(connection: &Connection) -> Result<ApplicationId, ApplicationStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map(ApplicationId::from)
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Persists an imported Application once, preserving the original specification on name conflicts.
pub(crate) fn insert_application(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    system_id: &SystemId,
    name: &ApplicationName,
    manifest_schema_version: u32,
) -> Result<bool, ApplicationStoreError> {
    let inserted = transaction
        .execute(
            "INSERT INTO applications (
                id, system_id, name, desired_runtime_state, spec_version,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, 'stopped', ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(name) DO NOTHING",
            params![
                application_id.as_str(),
                system_id.as_str(),
                name.as_str(),
                manifest_schema_version
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(inserted == 1)
}

// Loads the import-facing Application summary with optional source metadata.
pub(crate) fn load_application_for_import(
    transaction: &Transaction<'_>,
    name: &ApplicationName,
) -> Result<Option<ApplicationSummary>, ApplicationStoreError> {
    transaction
        .query_row(
            "SELECT
                a.id,
                a.system_id,
                a.name,
                a.desired_runtime_state,
                a.active_deployment_id,
                a.spec_version,
                s.repository_url,
                s.default_branch
             FROM applications AS a
             LEFT JOIN application_sources AS s
                ON s.application_id = a.id
             WHERE a.name = ?1",
            [name.as_str()],
            map_application_summary_row,
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Maps persisted Application state and rejects invalid desired-runtime values.
pub(crate) fn map_application_row(row: &Row<'_>) -> rusqlite::Result<Application> {
    let desired_runtime_state = row.get::<_, String>(3)?;
    let desired_runtime_state = desired_runtime_state_from_value(&desired_runtime_state)
        .ok_or_else(|| invalid_text_value(3, "desired runtime state", &desired_runtime_state))?;

    Ok(Application {
        id: ApplicationId::from(row.get::<_, String>(0)?),
        system_id: row.get::<_, Option<String>>(1)?.map(SystemId::from),
        name: ApplicationName::new(&row.get::<_, String>(2)?)
            .map_err(|error| invalid_text_value(2, "application name", &error.value))?,
        desired_runtime_state,
        active_deployment_id: row.get::<_, Option<String>>(4)?.map(DeploymentId::from),
        manifest_schema_version: row.get(5)?,
    })
}

// Extends the core Application row mapping with catalog source fields.
pub(crate) fn map_application_summary_row(row: &Row<'_>) -> rusqlite::Result<ApplicationSummary> {
    let application = map_application_row(row)?;
    Ok(ApplicationSummary {
        id: application.id,
        system_id: application.system_id,
        name: application.name,
        repository: row.get(6)?,
        default_branch: row.get(7)?,
        desired_runtime_state: application.desired_runtime_state,
        active_deployment_id: application.active_deployment_id,
        manifest_schema_version: application.manifest_schema_version,
    })
}

// Loads the core Application projection by its durable unique name.
pub fn load_application_by_name(
    connection: &Connection,
    name: &ApplicationName,
) -> Result<Option<Application>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT id, system_id, name, desired_runtime_state,
                    active_deployment_id, spec_version
             FROM applications WHERE name = ?1",
            [name.as_str()],
            map_application_row,
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Lists catalog summaries in stable display order.
pub(crate) fn list_application_summaries(
    connection: &Connection,
) -> Result<Vec<ApplicationSummary>, ApplicationStoreError> {
    let mut statement = connection.prepare("SELECT a.id, a.system_id, a.name, a.desired_runtime_state, a.active_deployment_id, a.spec_version, s.repository_url, s.default_branch FROM applications a LEFT JOIN application_sources s ON s.application_id = a.id ORDER BY a.name").map_err(|source| ApplicationStoreError::Persistence { source })?;
    statement
        .query_map([], map_application_summary_row)
        .map_err(|source| ApplicationStoreError::Persistence { source })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Lists catalog summaries belonging to one System.
pub(crate) fn list_application_summaries_for_system(
    connection: &Connection,
    system_id: &SystemId,
) -> Result<Vec<ApplicationSummary>, ApplicationStoreError> {
    let mut statement = connection.prepare("SELECT a.id, a.system_id, a.name, a.desired_runtime_state, a.active_deployment_id, a.spec_version, s.repository_url, s.default_branch FROM applications a LEFT JOIN application_sources s ON s.application_id = a.id WHERE a.system_id = ?1 ORDER BY a.name").map_err(|source| ApplicationStoreError::Persistence { source })?;
    statement
        .query_map([system_id.as_str()], map_application_summary_row)
        .map_err(|source| ApplicationStoreError::Persistence { source })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

pub(crate) fn application_has_successful_deployment(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<bool, ApplicationStoreError> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM deployments WHERE application_id = ?1 AND status = 'succeeded')", [application_id.as_str()], |row| row.get(0)).map_err(|source| ApplicationStoreError::Persistence { source })
}

// Loads persisted runtime intent from its owning Application aggregate.
pub(crate) fn load_desired_runtime_state(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<DesiredRuntimeState, ApplicationStoreError> {
    let value = connection
        .query_row(
            "SELECT desired_runtime_state FROM applications WHERE id = ?1",
            [application_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    desired_runtime_state_from_value(&value).ok_or_else(|| {
        ApplicationStoreError::InvalidDesiredRuntimeState {
            application_id: application_id.to_string(),
            state: value,
        }
    })
}

// Records the operator's runtime intent while the Application lock serializes its workflow.
pub(crate) fn set_desired_runtime_state(
    connection: &Connection,
    application_id: &ApplicationId,
    desired: DesiredRuntimeState,
) -> Result<(), ApplicationStoreError> {
    connection
        .execute(
            "UPDATE applications
             SET desired_runtime_state = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![
                desired_runtime_state_value(desired),
                application_id.as_str()
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

// Persists the immutable delivery configuration associated with an imported Application.
pub(crate) fn insert_delivery_spec(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    delivery_type: DeliveryType,
    image_repository: &OciRepository,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO application_delivery_specs (
                application_id, delivery_type, image_repository,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id.as_str(),
                delivery_type_value(delivery_type),
                image_repository.as_str()
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

// Persists source provenance and checkout defaults for an imported Application.
pub(crate) fn insert_source_spec(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    source: &ApplicationSource,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO application_sources (
                application_id, repository_url, repository_kind,
                default_branch, manifest_path, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id.as_str(),
                source.repository_location(),
                repository_kind_value(source.repository_kind()),
                source.default_branch(),
                source.manifest_path().as_str()
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

// Persists the container port that defines the Application's runtime endpoint.
pub(crate) fn insert_runtime_spec(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    container_port: ContainerPort,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO application_runtime_specs (
                application_id, container_port, created_at, updated_at
            ) VALUES (?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![application_id.as_str(), i64::from(container_port.get())],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

// Persists the internal health-check contract used for candidate verification.
pub(crate) fn insert_health_check_spec(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    path: &HealthCheckPath,
    expected_status: HealthCheckStatus,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO health_check_specs (
                application_id, path, expected_status, created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id.as_str(),
                path.as_str(),
                i64::from(expected_status.get())
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

// Checks durable Application existence before dependent persistence work.
pub(crate) fn application_exists(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<bool, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM applications WHERE id = ?1)",
            [application_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Atomically records the active Deployment and its running runtime intent, but only
// when the Deployment belongs to this Application and has already succeeded; anything
// else leaves the persisted state untouched and reports a stale outcome.
pub fn activate_deployment(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    deployment_id: &DeploymentId,
) -> Result<PersistenceOutcome, ApplicationStoreError> {
    let updated = transaction
        .execute(
            "UPDATE applications
             SET active_deployment_id = ?1,
                 desired_runtime_state = 'running',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2
               AND EXISTS (
                   SELECT 1 FROM deployments
                   WHERE deployments.id = ?1
                     AND deployments.application_id = applications.id
                     AND deployments.status = 'succeeded'
               )",
            params![deployment_id.as_str(), application_id.as_str()],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Loads delivery configuration and maps its persisted type into the domain enum.
pub fn load_delivery_specification(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Option<DeliverySpecification>, ApplicationStoreError> {
    let specification = connection
        .query_row(
            "SELECT delivery_type, image_repository
             FROM application_delivery_specs WHERE application_id = ?1",
            [application_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    let Some((delivery_type, image_repository)) = specification else {
        return Ok(None);
    };
    let delivery_type = delivery_type_from_value(&delivery_type).ok_or_else(|| {
        ApplicationStoreError::Persistence {
            source: invalid_text_value(0, "delivery type", &delivery_type),
        }
    })?;
    let image_repository = OciRepository::new(&image_repository).map_err(|error| {
        ApplicationStoreError::Persistence {
            source: invalid_text_value(1, "OCI repository", &error.repository),
        }
    })?;
    Ok(Some(DeliverySpecification::new(
        delivery_type,
        image_repository,
    )))
}

// Loads source configuration and rejects unknown persisted repository kinds.
pub fn load_source(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Option<ApplicationSource>, ApplicationStoreError> {
    let source = connection
        .query_row(
            "SELECT repository_url, repository_kind, default_branch, manifest_path
             FROM application_sources WHERE application_id = ?1",
            [application_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    let Some((repository_url, repository_kind, default_branch, manifest_path)) = source else {
        return Ok(None);
    };
    let repository_kind = repository_kind_from_value(&repository_kind).ok_or_else(|| {
        ApplicationStoreError::Persistence {
            source: invalid_text_value(1, "repository kind", &repository_kind),
        }
    })?;
    let manifest_path = RelativeManifestPath::new(&manifest_path).map_err(|error| {
        ApplicationStoreError::Persistence {
            source: invalid_text_value(3, "manifest path", &error.path),
        }
    })?;
    ApplicationSource::new(
        repository_kind,
        &repository_url,
        default_branch,
        manifest_path,
    )
    .map(Some)
    .map_err(|_| ApplicationStoreError::Persistence {
        source: invalid_text_value(0, "repository location", &repository_url),
    })
}

// Joins persisted runtime, health, and visibility data into deployment input.
pub fn load_deployment_specification(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Option<ApplicationDeploymentSpecification>, ApplicationStoreError> {
    let specification = connection
        .query_row(
            "SELECT
                applications.id,
                applications.name,
                application_runtime_specs.container_port,
                health_check_specs.path,
                health_check_specs.expected_status,
                exposures.desired_visibility
             FROM applications
             JOIN application_runtime_specs
                ON application_runtime_specs.application_id = applications.id
             JOIN health_check_specs
                ON health_check_specs.application_id = applications.id
             JOIN exposures ON exposures.application_id = applications.id
             WHERE applications.id = ?1",
            [application_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u16>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u16>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    let Some((application_id, application_name, container_port, path, expected_status, visibility)) =
        specification
    else {
        return Ok(None);
    };
    let visibility =
        visibility_from_value(&visibility).ok_or_else(|| ApplicationStoreError::Persistence {
            source: invalid_text_value(5, "visibility", &visibility),
        })?;
    Ok(Some(ApplicationDeploymentSpecification {
        application_id: ApplicationId::from(application_id),
        application_name: ApplicationName::new(&application_name).map_err(|error| {
            ApplicationStoreError::Persistence {
                source: invalid_text_value(1, "application name", &error.value),
            }
        })?,
        runtime: RuntimeSpecification::new(
            ContainerPort::new(container_port).map_err(|error| {
                ApplicationStoreError::Persistence {
                    source: invalid_text_value(2, "container port", &error.value.to_string()),
                }
            })?,
            HealthCheckSpecification::new(
                HealthCheckPath::new(&path).map_err(|error| {
                    ApplicationStoreError::Persistence {
                        source: invalid_text_value(3, "health check path", &error.value),
                    }
                })?,
                HealthCheckStatus::new(expected_status).map_err(|error| {
                    ApplicationStoreError::Persistence {
                        source: invalid_text_value(
                            4,
                            "health check status",
                            &error.value.to_string(),
                        ),
                    }
                })?,
            ),
        ),
        visibility,
    }))
}

fn delivery_type_value(value: DeliveryType) -> &'static str {
    match value {
        DeliveryType::Oci => "oci",
    }
}
fn delivery_type_from_value(value: &str) -> Option<DeliveryType> {
    match value {
        "oci" => Some(DeliveryType::Oci),
        _ => None,
    }
}
fn repository_kind_value(value: RepositoryKind) -> &'static str {
    match value {
        RepositoryKind::Local => "local",
        RepositoryKind::Remote => "remote",
    }
}
fn repository_kind_from_value(value: &str) -> Option<RepositoryKind> {
    match value {
        "local" => Some(RepositoryKind::Local),
        "remote" => Some(RepositoryKind::Remote),
        _ => None,
    }
}
fn desired_runtime_state_from_value(value: &str) -> Option<DesiredRuntimeState> {
    match value {
        "running" => Some(DesiredRuntimeState::Running),
        "stopped" => Some(DesiredRuntimeState::Stopped),
        _ => None,
    }
}
fn desired_runtime_state_value(value: DesiredRuntimeState) -> &'static str {
    match value {
        DesiredRuntimeState::Running => "running",
        DesiredRuntimeState::Stopped => "stopped",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::{Connection, TransactionBehavior, params};

    use crate::adapters::database;
    use crate::domain::application::ApplicationName;
    use crate::domain::identity::{ApplicationId, SystemId};

    use super::{
        ApplicationStoreError, DesiredRuntimeState, insert_application, insert_delivery_spec,
        insert_runtime_spec, load_application_by_name, load_application_for_import,
        load_delivery_specification, load_deployment_specification, load_desired_runtime_state,
        load_source,
    };

    fn application_id() -> ApplicationId {
        ApplicationId::from("app")
    }

    #[test]
    fn absent_lookups_are_ok_none_never_a_not_found_error() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        let name = ApplicationName::new("missing").unwrap();

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            load_application_by_name(&transaction, &name).unwrap(),
            None,
            "absence must be Ok(None): this store has no NotFound variant"
        );
        assert_eq!(
            load_application_for_import(&transaction, &name).unwrap(),
            None
        );
        assert_eq!(
            load_delivery_specification(&transaction, &application_id()).unwrap(),
            None
        );
        assert_eq!(load_source(&transaction, &application_id()).unwrap(), None);
        assert_eq!(
            load_deployment_specification(&transaction, &application_id()).unwrap(),
            None
        );
    }

    fn seed_application(connection: &Connection, id: &str) {
        connection
            .execute(
                "INSERT INTO applications (id, name, desired_runtime_state, spec_version, created_at, updated_at)
                 VALUES (?1, ?1, 'stopped', 3, 'now', 'now')",
                params![id],
            )
            .unwrap();
    }

    #[test]
    fn desired_runtime_state_records_the_locked_lifecycle_intent() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "app");

        super::set_desired_runtime_state(
            &connection,
            &application_id(),
            DesiredRuntimeState::Running,
        )
        .unwrap();
        assert_eq!(
            load_desired_runtime_state(&connection, &application_id()).unwrap(),
            DesiredRuntimeState::Running
        );
    }

    #[test]
    fn corrupt_desired_runtime_state_is_a_typed_error_not_an_invented_state() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "app");
        // The CHECK constraint is bypassed so a corrupt historical row can exist.
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        connection
            .execute(
                "UPDATE applications SET desired_runtime_state = 'paused' WHERE id = 'app'",
                [],
            )
            .unwrap();

        let error = load_desired_runtime_state(&connection, &application_id()).unwrap_err();

        assert!(matches!(
            error,
            ApplicationStoreError::InvalidDesiredRuntimeState { state, .. } if state == "paused"
        ));
    }

    #[test]
    fn rolling_back_the_import_transaction_persists_nothing() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        let repository =
            crate::domain::release::OciRepository::new("registry.example/app").unwrap();

        {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            transaction
                .execute(
                    "INSERT INTO systems (id, name, created_at) VALUES ('system-id', 'team', 'now')",
                    [],
                )
                .unwrap();
            insert_application(
                &transaction,
                &application_id(),
                &SystemId::from("system-id"),
                &ApplicationName::new("app").unwrap(),
                3,
            )
            .unwrap();
            insert_delivery_spec(
                &transaction,
                &application_id(),
                crate::domain::release::DeliveryType::Oci,
                &repository,
            )
            .unwrap();
            insert_runtime_spec(
                &transaction,
                &application_id(),
                crate::domain::runtime::ContainerPort::new(8080).unwrap(),
            )
            .unwrap();
            drop(transaction);
        }

        for table in [
            "applications",
            "application_delivery_specs",
            "application_runtime_specs",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} must stay empty after a rollback");
        }
    }

    #[test]
    fn unknown_enum_text_in_specification_rows_is_rejected_with_context() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "app");

        // The delivery type CHECK is bypassed so a corrupt historical row can exist.
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        connection
            .execute(
                "INSERT INTO application_delivery_specs (
                    application_id, delivery_type, image_repository, created_at, updated_at
                 ) VALUES ('app', 'docker', 'registry.example/app', 'now', 'now')",
                [],
            )
            .unwrap();

        let error = load_delivery_specification(&connection, &application_id());

        assert!(matches!(
            error,
            Err(ApplicationStoreError::Persistence { .. })
        ));
    }

    #[test]
    fn unknown_repository_kind_is_rejected_when_loading_the_source() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "app");

        // The repository kind CHECK is bypassed so a corrupt historical row can exist.
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        connection
            .execute(
                "INSERT INTO application_sources (
                    application_id, repository_url, repository_kind,
                    default_branch, manifest_path, created_at, updated_at
                 ) VALUES ('app', 'https://github.com/example/app', 'svn',
                           'main', 'pneuma.toml', 'now', 'now')",
                [],
            )
            .unwrap();

        let error = load_source(&connection, &application_id());

        assert!(matches!(
            error,
            Err(ApplicationStoreError::Persistence { .. })
        ));
    }

    #[test]
    fn unknown_visibility_text_is_rejected_when_loading_the_deployment_specification() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed_application(&connection, "app");
        connection
            .execute_batch(
                "PRAGMA ignore_check_constraints = ON;
                 INSERT INTO application_runtime_specs (
                    application_id, container_port, created_at, updated_at
                 ) VALUES ('app', 8080, 'now', 'now');
                 INSERT INTO health_check_specs (
                    application_id, path, expected_status, created_at, updated_at
                 ) VALUES ('app', '/healthz', 200, 'now', 'now');
                 INSERT INTO exposures (
                    application_id, desired_visibility, domain, created_at, updated_at
                 ) VALUES ('app', 'private', NULL, 'now', 'now');",
            )
            .unwrap();

        let error = load_deployment_specification(&connection, &application_id());

        assert!(matches!(
            error,
            Err(ApplicationStoreError::Persistence { .. })
        ));
    }
}
