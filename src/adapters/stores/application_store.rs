use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::domain::application::Application;
use crate::domain::manifest::{DeliveryType, Visibility};

#[derive(Debug)]
pub enum ApplicationStoreError {
    NotFound { application_id: String },
    SystemNotFound { system_name: String },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for ApplicationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { application_id } => {
                write!(formatter, "application `{application_id}` not found")
            }
            Self::SystemNotFound { system_name } => {
                write!(formatter, "system `{system_name}` not found")
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
            Self::NotFound { .. } | Self::SystemNotFound { .. } => None,
        }
    }
}

pub fn generate_id(connection: &Connection) -> Result<String, ApplicationStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

pub fn ensure_system(
    transaction: &Transaction<'_>,
    system_id: &str,
    system_name: &str,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO systems (id, name, created_at)
             VALUES (?1, ?2, CURRENT_TIMESTAMP)
             ON CONFLICT(name) DO NOTHING",
            params![system_id, system_name],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn load_system_id_by_name(
    transaction: &Transaction<'_>,
    system_name: &str,
) -> Result<String, ApplicationStoreError> {
    transaction
        .query_row(
            "SELECT id FROM systems WHERE name = ?1",
            [system_name],
            |row| row.get(0),
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

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

pub fn load_application_for_import(
    transaction: &Transaction<'_>,
    name: &str,
) -> Result<Option<Application>, ApplicationStoreError> {
    transaction
        .query_row(
            "SELECT
                a.id,
                a.system_id,
                a.name,
                s.repository_url,
                s.default_branch,
                a.active_deployment_id
             FROM applications AS a
             LEFT JOIN application_sources AS s
                ON s.application_id = a.id
             WHERE a.name = ?1",
            [name],
            |row| {
                Ok(Application {
                    id: row.get(0)?,
                    system_id: row.get(1)?,
                    name: row.get(2)?,
                    repository: row.get(3)?,
                    default_branch: row.get(4)?,
                    active_deployment_id: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

pub fn insert_delivery_spec(
    transaction: &Transaction<'_>,
    application_id: &str,
    delivery_type: DeliveryType,
    image_repository: &str,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO application_delivery_specs (
                application_id, delivery_type, image_repository,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                delivery_type.database_value(),
                image_repository
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn insert_source_spec(
    transaction: &Transaction<'_>,
    application_id: &str,
    repository_url: &str,
    repository_kind: &str,
    default_branch: Option<&str>,
    manifest_path: &str,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO application_sources (
                application_id, repository_url, repository_kind,
                default_branch, manifest_path, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                repository_url,
                repository_kind,
                default_branch,
                manifest_path
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn insert_runtime_spec(
    transaction: &Transaction<'_>,
    application_id: &str,
    container_port: u16,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO application_runtime_specs (
                application_id, container_port, created_at, updated_at
            ) VALUES (?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![application_id, i64::from(container_port)],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn insert_health_check_spec(
    transaction: &Transaction<'_>,
    application_id: &str,
    path: &str,
    expected_status: u16,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO health_check_specs (
                application_id, path, expected_status, created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![application_id, path, i64::from(expected_status)],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn insert_exposure(
    transaction: &Transaction<'_>,
    application_id: &str,
    visibility: Visibility,
    domain: Option<&str>,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO exposures (
                application_id, desired_visibility, domain,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![application_id, visibility.database_value(), domain],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

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

pub fn load_desired_runtime_state(
    connection: &Connection,
    application_id: &str,
) -> Result<String, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT desired_runtime_state FROM applications WHERE id = ?1",
            [application_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })?
        .ok_or_else(|| ApplicationStoreError::NotFound {
            application_id: application_id.to_owned(),
        })
}

pub fn update_desired_runtime_state(
    connection: &Connection,
    application_id: &str,
    state: &str,
) -> Result<(), ApplicationStoreError> {
    connection
        .execute(
            "UPDATE applications
             SET desired_runtime_state = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![state, application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn activate_deployment(
    transaction: &Transaction<'_>,
    application_id: &str,
    deployment_id: &str,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "UPDATE applications
             SET active_deployment_id = ?1,
                 desired_runtime_state = 'running',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![deployment_id, application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn load_delivery_image_repository(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<String>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT image_repository FROM application_delivery_specs WHERE application_id = ?1",
            [application_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

pub fn load_source_repository(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<(String, Option<String>)>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT repository_url, default_branch FROM application_sources WHERE application_id = ?1",
            [application_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

pub fn load_exposure_visibility(
    connection: &Connection,
    application_id: &str,
) -> Result<String, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT desired_visibility FROM exposures WHERE application_id = ?1",
            [application_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })?
        .ok_or_else(|| ApplicationStoreError::NotFound {
            application_id: application_id.to_owned(),
        })
}

pub fn load_exposure_domain(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<String>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT domain FROM exposures WHERE application_id = ?1",
            [application_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

pub fn update_exposure_public(
    connection: &Connection,
    application_id: &str,
    domain: &str,
) -> Result<(), ApplicationStoreError> {
    connection
        .execute(
            "UPDATE exposures
             SET desired_visibility = 'public',
                 domain = ?1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?2",
            params![domain, application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn update_exposure_internal(
    connection: &Connection,
    application_id: &str,
) -> Result<(), ApplicationStoreError> {
    connection
        .execute(
            "UPDATE exposures
             SET desired_visibility = 'internal', updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1",
            [application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn begin_public_exposure(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<bool, ApplicationStoreError> {
    let updated = transaction
        .execute(
            "UPDATE exposures
             SET materialization_state = 'applying',
                 last_materialized_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1 AND desired_visibility = 'public'",
            [application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn record_exposure_failure(
    transaction: &Transaction<'_>,
    application_id: &str,
    state: &str,
    code: &str,
    message: &str,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "UPDATE exposures
             SET materialization_state = ?1,
                 last_error_code = ?2,
                 last_error_message = ?3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?4 AND desired_visibility = 'public'",
            params![state, code, message, application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(())
}

pub fn complete_public_exposure(
    transaction: &Transaction<'_>,
    application_id: &str,
    runtime_id: &str,
    configuration_version: &str,
) -> Result<bool, ApplicationStoreError> {
    let updated = transaction
        .execute(
            "UPDATE exposures
             SET active_runtime_id = ?1,
                 materialization_state = 'active',
                 configuration_version = ?2,
                 last_materialized_at = CURRENT_TIMESTAMP,
                 last_error_code = NULL,
                 last_error_message = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?3
               AND desired_visibility = 'public'
               AND materialization_state = 'applying'",
            params![runtime_id, configuration_version, application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn load_exposure_for_runtime(
    connection: &Connection,
    runtime_id: &str,
) -> Result<Option<(String, String, String, String)>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT e.application_id, e.desired_visibility, e.domain, e.materialization_state
             FROM exposures e
             JOIN runtime_instances ri ON ri.deployment_id = (
                 SELECT active_deployment_id FROM applications WHERE id = e.application_id
             )
             WHERE ri.id = ?1",
            [runtime_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

pub fn load_runtime_endpoint_for_exposure(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<(String, u16, Option<String>)>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT ri.external_runtime_id, ri.container_port, e.domain
             FROM runtime_instances ri
             JOIN applications a ON a.active_deployment_id = ri.deployment_id
             JOIN exposures e ON e.application_id = a.id
             WHERE a.id = ?1 AND ri.state = 'running' AND ri.removed_at IS NULL",
            [application_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

#[allow(clippy::type_complexity)]
pub fn load_deployment_specification(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<(String, String, u16, String, u16, String)>, ApplicationStoreError> {
    connection
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
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|source| ApplicationStoreError::Persistence { source })
}
