use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use thiserror::Error;

use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::persistence::{
    entity_id, invalid_text_value, outcome, visibility_from_value,
};
use crate::domain::application::{
    Application, ApplicationDeploymentSpecification, ApplicationName, ApplicationSummary,
    DesiredRuntimeState,
};
use crate::domain::git::ApplicationSource;
use crate::domain::identity::{ApplicationId, DeploymentId, SystemId};
use crate::domain::release::{DeliverySpecification, OciRepository};
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
    let value = connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    ApplicationId::new(&value).map_err(|_| ApplicationStoreError::Persistence {
        source: invalid_text_value(0, "application id", &value),
    })
}

// The complete immutable import specification persisted on one Application row.
pub(crate) struct ImportedApplicationSpecification<'a> {
    pub(crate) system_id: &'a SystemId,
    pub(crate) name: &'a ApplicationName,
    pub(crate) source: &'a ApplicationSource,
    pub(crate) image_repository: &'a OciRepository,
    pub(crate) runtime: &'a RuntimeSpecification,
}

// Persists an imported Application once, preserving the original specification on name conflicts.
pub(crate) fn insert_application(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    specification: &ImportedApplicationSpecification<'_>,
) -> Result<bool, ApplicationStoreError> {
    let health_check = specification.runtime.health_check();
    let inserted = transaction
        .execute(
            "INSERT INTO applications (
                id, system_id, name,
                repository_url, default_branch, manifest_path,
                image_repository, container_port,
                health_check_path, health_check_expected_status,
                desired_runtime_state
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'stopped')
            ON CONFLICT(name) DO NOTHING",
            params![
                application_id.as_str(),
                specification.system_id.as_str(),
                specification.name.as_str(),
                specification.source.repository_url(),
                specification.source.default_branch(),
                specification.source.manifest_path().as_str(),
                specification.image_repository.as_str(),
                i64::from(specification.runtime.container_port().get()),
                health_check.path().as_str(),
                i64::from(health_check.expected_status().get()),
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(inserted == 1)
}

// Loads the import-facing Application summary with its immutable source metadata.
pub(crate) fn load_application_for_import(
    transaction: &Transaction<'_>,
    name: &ApplicationName,
) -> Result<Option<ApplicationSummary>, ApplicationStoreError> {
    transaction
        .query_row(
            "SELECT id, system_id, name, desired_runtime_state, active_deployment_id,
                    repository_url, default_branch
             FROM applications WHERE name = ?1",
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
        id: entity_id(0, &row.get::<_, String>(0)?)?,
        system_id: entity_id(1, &row.get::<_, String>(1)?)?,
        name: ApplicationName::new(&row.get::<_, String>(2)?)
            .map_err(|error| invalid_text_value(2, "application name", &error.value))?,
        desired_runtime_state,
        active_deployment_id: row
            .get::<_, Option<String>>(4)?
            .map(|value| entity_id(4, &value))
            .transpose()?,
    })
}

// Extends the core Application row mapping with catalog source fields.
pub(crate) fn map_application_summary_row(row: &Row<'_>) -> rusqlite::Result<ApplicationSummary> {
    let application = map_application_row(row)?;
    Ok(ApplicationSummary {
        id: application.id,
        system_id: application.system_id,
        name: application.name,
        repository: row.get(5)?,
        default_branch: row.get(6)?,
        desired_runtime_state: application.desired_runtime_state,
        active_deployment_id: application.active_deployment_id,
    })
}

// Loads the core Application projection by its durable unique name.
pub fn load_application_by_name(
    connection: &Connection,
    name: &ApplicationName,
) -> Result<Option<Application>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT id, system_id, name, desired_runtime_state, active_deployment_id
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
    let mut statement = connection.prepare("SELECT id, system_id, name, desired_runtime_state, active_deployment_id, repository_url, default_branch FROM applications ORDER BY name").map_err(|source| ApplicationStoreError::Persistence { source })?;
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
    let mut statement = connection.prepare("SELECT id, system_id, name, desired_runtime_state, active_deployment_id, repository_url, default_branch FROM applications WHERE system_id = ?1 ORDER BY name").map_err(|source| ApplicationStoreError::Persistence { source })?;
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
            "UPDATE applications SET desired_runtime_state = ?1 WHERE id = ?2",
            params![
                desired_runtime_state_value(desired),
                application_id.as_str()
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
                 desired_runtime_state = 'running'
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

// Loads the immutable delivery configuration persisted on the Application row.
pub fn load_delivery_specification(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Option<DeliverySpecification>, ApplicationStoreError> {
    let image_repository = connection
        .query_row(
            "SELECT image_repository FROM applications WHERE id = ?1",
            [application_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    let Some(image_repository) = image_repository else {
        return Ok(None);
    };
    let image_repository = OciRepository::new(&image_repository).map_err(|error| {
        ApplicationStoreError::Persistence {
            source: invalid_text_value(0, "OCI repository", &error.repository),
        }
    })?;
    Ok(Some(DeliverySpecification::new(image_repository)))
}

// Loads the immutable remote Git source persisted on the Application row and
// rejects locations outside the remote Git form.
pub fn load_source(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<Option<ApplicationSource>, ApplicationStoreError> {
    let source = connection
        .query_row(
            "SELECT repository_url, default_branch, manifest_path
             FROM applications WHERE id = ?1",
            [application_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    let Some((repository_url, default_branch, manifest_path)) = source else {
        return Ok(None);
    };
    let manifest_path =
        crate::domain::git::RelativeManifestPath::new(&manifest_path).map_err(|error| {
            ApplicationStoreError::Persistence {
                source: invalid_text_value(2, "manifest path", &error.path),
            }
        })?;
    ApplicationSource::new(&repository_url, default_branch, manifest_path)
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
                applications.container_port,
                applications.health_check_path,
                applications.health_check_expected_status,
                exposures.desired_visibility
             FROM applications
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
        application_id: entity_id(0, &application_id)
            .map_err(|source| ApplicationStoreError::Persistence { source })?,
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
    use crate::domain::git::{ApplicationSource, RelativeManifestPath};
    use crate::domain::identity::{ApplicationId, SystemId};
    use crate::domain::release::OciRepository;
    use crate::domain::runtime::{
        ContainerPort, HealthCheckPath, HealthCheckSpecification, HealthCheckStatus,
        RuntimeSpecification,
    };

    use super::{
        ApplicationStoreError, DesiredRuntimeState, ImportedApplicationSpecification,
        insert_application, load_application_by_name, load_application_for_import,
        load_delivery_specification, load_deployment_specification, load_desired_runtime_state,
        load_source,
    };

    const APP_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SYSTEM_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn application_id() -> ApplicationId {
        ApplicationId::new(APP_ID).unwrap()
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

    fn specification(name: &ApplicationName) -> ImportedApplicationSpecification<'static> {
        specification_leaked(name)
    }

    // Builds a specification with `'static` borrowed fields for test convenience.
    fn specification_leaked(name: &ApplicationName) -> ImportedApplicationSpecification<'static> {
        let system_id = Box::leak(SystemId::new(SYSTEM_ID).unwrap().into());
        let source = Box::leak(Box::new(
            ApplicationSource::new(
                "https://example.test/app.git",
                Some("main".to_owned()),
                RelativeManifestPath::new("pneuma.toml").unwrap(),
            )
            .unwrap(),
        ));
        let image_repository =
            Box::leak(OciRepository::new("registry.example/app").unwrap().into());
        let runtime = Box::leak(Box::new(RuntimeSpecification::new(
            ContainerPort::new(8080).unwrap(),
            HealthCheckSpecification::new(
                HealthCheckPath::new("/healthz").unwrap(),
                HealthCheckStatus::new(200).unwrap(),
            ),
        )));
        ImportedApplicationSpecification {
            system_id,
            name: Box::leak(name.clone().into()),
            source,
            image_repository,
            runtime,
        }
    }

    #[test]
    fn desired_runtime_state_records_the_locked_lifecycle_intent() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed_application(&connection, APP_ID);

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
        seed_application(&connection, APP_ID);
        // The CHECK constraint is bypassed so a corrupt historical row can exist.
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        connection
            .execute(
                "UPDATE applications SET desired_runtime_state = 'paused' WHERE id = ?1",
                params![APP_ID],
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
        let name = ApplicationName::new("app").unwrap();

        {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            transaction
                .execute(
                    "INSERT INTO systems (id, name) VALUES (?1, 'team')",
                    params![SYSTEM_ID],
                )
                .unwrap();
            insert_application(&transaction, &application_id(), &specification(&name)).unwrap();
            drop(transaction);
        }

        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "applications must stay empty after a rollback");
    }

    #[test]
    fn import_persists_the_whole_specification_on_one_row() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        let name = ApplicationName::new("app").unwrap();
        connection
            .execute(
                "INSERT INTO systems (id, name) VALUES (?1, 'team')",
                params![SYSTEM_ID],
            )
            .unwrap();

        {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(
                insert_application(&transaction, &application_id(), &specification(&name)).unwrap()
            );
            super::super::exposure_store::insert_exposure(
                &transaction,
                &application_id(),
                &crate::domain::exposure::ExposureIntent::new(
                    crate::domain::exposure::Visibility::Internal,
                    None,
                )
                .unwrap(),
            )
            .unwrap();
            transaction.commit().unwrap();
        }

        let source = load_source(&connection, &application_id())
            .unwrap()
            .expect("the imported source must load");
        assert_eq!(source.repository_url(), "https://example.test/app.git");
        assert_eq!(source.default_branch(), Some("main"));
        assert_eq!(source.manifest_path().as_str(), "pneuma.toml");

        let delivery = load_delivery_specification(&connection, &application_id())
            .unwrap()
            .expect("the imported delivery must load");
        assert_eq!(delivery.image_repository().as_str(), "registry.example/app");

        let deployment_specification =
            load_deployment_specification(&connection, &application_id())
                .unwrap()
                .expect("the deployment specification must load");
        assert_eq!(
            deployment_specification.runtime.container_port().get(),
            8080
        );
        assert_eq!(
            deployment_specification
                .runtime
                .health_check()
                .path()
                .as_str(),
            "/healthz"
        );
    }

    #[test]
    fn applications_without_a_system_cannot_exist() {
        let connection = database::open(Path::new(":memory:")).unwrap();

        // The schema makes a System required; an obsolete row without one is
        // unrepresentable instead of being tolerated at hydration.
        let error = connection
            .execute(
                "INSERT INTO applications (
                     id, system_id, name, repository_url, manifest_path, image_repository,
                     container_port, health_check_path, health_check_expected_status,
                     desired_runtime_state
                 ) VALUES (?1, NULL, 'app', 'https://example.test/app.git', 'pneuma.toml',
                           'registry.example/app', 8080, '/healthz', 200, 'stopped')",
                params![APP_ID],
            )
            .unwrap_err();

        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    #[test]
    fn unknown_visibility_text_is_rejected_when_loading_the_deployment_specification() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        seed_application(&connection, APP_ID);
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        connection
            .execute_batch(&format!(
                "INSERT INTO exposures (
                     application_id, desired_visibility, domain, materialization_state
                 ) VALUES ('{APP_ID}', 'private', NULL, 'not_materialized');"
            ))
            .unwrap();

        let error = load_deployment_specification(&connection, &application_id());

        assert!(matches!(
            error,
            Err(ApplicationStoreError::Persistence { .. })
        ));
    }

    fn seed_application(connection: &Connection, id: &str) {
        connection
            .execute_batch(&format!(
                "INSERT INTO systems (id, name) VALUES ('{SYSTEM_ID}', 'team');
                 INSERT INTO applications (
                     id, system_id, name, repository_url, default_branch, manifest_path,
                     image_repository, container_port, health_check_path,
                     health_check_expected_status, desired_runtime_state
                 ) VALUES (
                     '{id}', '{SYSTEM_ID}', '{id}', 'https://example.test/app.git', 'main',
                     'pneuma.toml', 'registry.example/app', 8080, '/healthz', 200, 'stopped')"
            ))
            .unwrap();
    }
}
