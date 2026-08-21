use std::error::Error;
use std::fmt;
use std::io;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::adapters::stores::PersistenceOutcome;
use crate::domain::application::{
    Application, ApplicationDeploymentSpecification, ApplicationName, ApplicationSource,
    ApplicationSummary, DesiredRuntimeState, RelativeManifestPath, RepositoryKind,
};
use crate::domain::delivery::{DeliverySpecification, DeliveryType};
use crate::domain::exposure::Visibility;
use crate::domain::identity::{ApplicationId, DeploymentId, SystemId};
use crate::domain::release::OciRepository;
use crate::domain::runtime::{
    ContainerPort, HealthCheckPath, HealthCheckSpecification, HealthCheckStatus,
    RuntimeSpecification,
};

#[derive(Debug)]
pub enum ApplicationStoreError {
    NotFound {
        application_id: String,
    },
    InvalidDesiredRuntimeState {
        application_id: String,
        state: String,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for ApplicationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { application_id } => {
                write!(formatter, "application `{application_id}` not found")
            }
            Self::InvalidDesiredRuntimeState {
                application_id,
                state,
            } => {
                write!(
                    formatter,
                    "application `{application_id}` has invalid desired runtime state `{state}`"
                )
            }
            Self::Persistence { source } => {
                write!(formatter, "application store error: {source}")
            }
        }
    }
}

impl Error for ApplicationStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Persistence { source } => Some(source),
            Self::NotFound { .. } | Self::InvalidDesiredRuntimeState { .. } => None,
        }
    }
}

// Allocates an ID inside the import transaction so related Application records share one boundary.
pub fn generate_id(connection: &Connection) -> Result<String, ApplicationStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Persists an imported Application once, preserving the original specification on name conflicts.
pub fn insert_application(
    transaction: &Transaction<'_>,
    application_id: &str,
    system_id: &str,
    name: &str,
    spec_version: u32,
) -> Result<bool, ApplicationStoreError> {
    let inserted = transaction
        .execute(
            "INSERT INTO applications (
                id, system_id, name, desired_runtime_state, spec_version,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, 'stopped', ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(name) DO NOTHING",
            params![application_id, system_id, name, spec_version],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(inserted == 1)
}

// Loads the import-facing Application summary with optional source metadata.
pub fn load_application_for_import(
    transaction: &Transaction<'_>,
    name: &str,
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
            [name],
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
        specification_version: row.get(5)?,
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
        specification_version: application.specification_version,
    })
}

// Loads the core Application projection by its durable unique name.
pub fn load_application_by_name(
    connection: &Connection,
    name: &str,
) -> Result<Option<Application>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT id, system_id, name, desired_runtime_state,
                    active_deployment_id, spec_version
             FROM applications WHERE name = ?1",
            [name],
            map_application_row,
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Lists catalog summaries in stable display order.
pub fn list_application_summaries(
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
pub fn list_application_summaries_for_system(
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

pub fn application_has_successful_deployment(
    connection: &Connection,
    application_id: &str,
) -> Result<bool, ApplicationStoreError> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM deployments WHERE application_id = ?1 AND status = 'succeeded')", [application_id], |row| row.get(0)).map_err(|source| ApplicationStoreError::Persistence { source })
}

// Loads persisted runtime intent from its owning Application aggregate.
pub fn load_desired_runtime_state(
    connection: &Connection,
    application_id: &str,
) -> Result<DesiredRuntimeState, ApplicationStoreError> {
    let value = connection
        .query_row(
            "SELECT desired_runtime_state FROM applications WHERE id = ?1",
            [application_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    desired_runtime_state_from_value(&value).ok_or_else(|| {
        ApplicationStoreError::InvalidDesiredRuntimeState {
            application_id: application_id.to_owned(),
            state: value,
        }
    })
}

// Changes Application runtime intent only when the persisted state matches the observation.
pub fn compare_and_set_desired_runtime_state(
    connection: &Connection,
    application_id: &str,
    expected: DesiredRuntimeState,
    desired: DesiredRuntimeState,
) -> Result<PersistenceOutcome, ApplicationStoreError> {
    let updated = connection
        .execute(
            "UPDATE applications
             SET desired_runtime_state = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2 AND desired_runtime_state = ?3",
            params![
                desired_runtime_state_value(desired),
                application_id,
                desired_runtime_state_value(expected)
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Persists the immutable delivery configuration associated with an imported Application.
pub fn insert_delivery_spec(
    transaction: &Transaction<'_>,
    application_id: &str,
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
                application_id,
                delivery_type_value(delivery_type),
                image_repository.as_str()
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

// Persists source provenance and checkout defaults for an imported Application.
pub fn insert_source_spec(
    transaction: &Transaction<'_>,
    application_id: &str,
    source: &ApplicationSource,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO application_sources (
                application_id, repository_url, repository_kind,
                default_branch, manifest_path, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
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
pub fn insert_runtime_spec(
    transaction: &Transaction<'_>,
    application_id: &str,
    container_port: ContainerPort,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO application_runtime_specs (
                application_id, container_port, created_at, updated_at
            ) VALUES (?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![application_id, i64::from(container_port.get())],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

// Persists the internal health-check contract used for candidate verification.
pub fn insert_health_check_spec(
    transaction: &Transaction<'_>,
    application_id: &str,
    path: &HealthCheckPath,
    expected_status: HealthCheckStatus,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO health_check_specs (
                application_id, path, expected_status, created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                path.as_str(),
                i64::from(expected_status.get())
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

// Checks durable Application existence before dependent persistence work.
pub fn application_exists(
    connection: &Connection,
    application_id: &str,
) -> Result<bool, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM applications WHERE id = ?1)",
            [application_id],
            |row| row.get(0),
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Atomically records the active Deployment and its running runtime intent.
pub fn activate_deployment(
    transaction: &Transaction<'_>,
    application_id: &str,
    deployment_id: &str,
) -> Result<PersistenceOutcome, ApplicationStoreError> {
    let updated = transaction
        .execute(
            "UPDATE applications
             SET active_deployment_id = ?1,
                 desired_runtime_state = 'running',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![deployment_id, application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Loads delivery configuration and maps its persisted type into the domain enum.
pub fn load_delivery_specification(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<DeliverySpecification>, ApplicationStoreError> {
    let specification = connection
        .query_row(
            "SELECT delivery_type, image_repository
             FROM application_delivery_specs WHERE application_id = ?1",
            [application_id],
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
    application_id: &str,
) -> Result<Option<ApplicationSource>, ApplicationStoreError> {
    let source = connection
        .query_row(
            "SELECT repository_url, repository_kind, default_branch, manifest_path
             FROM application_sources WHERE application_id = ?1",
            [application_id],
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
    application_id: &str,
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
            [application_id],
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

// Converts an invalid persisted text value into a row-mapping error with column context.
fn invalid_text_value(column: usize, field: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {field}: {value}"),
        )),
    )
}
fn outcome(updated: usize) -> PersistenceOutcome {
    if updated == 1 {
        PersistenceOutcome::Updated
    } else {
        PersistenceOutcome::Stale
    }
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
fn visibility_from_value(value: &str) -> Option<Visibility> {
    match value {
        "internal" => Some(Visibility::Internal),
        "public" => Some(Visibility::Public),
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
