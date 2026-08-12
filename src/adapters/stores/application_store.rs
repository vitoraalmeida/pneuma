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

#[derive(Debug, PartialEq, Eq)]
pub struct StoredExposure {
    pub visibility: Visibility,
    pub domain: Option<String>,
    pub materialization_state: String,
}

#[derive(Debug)]
pub enum ExposureStoreError {
    InvalidVisibility {
        application_id: String,
        visibility: String,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for ExposureStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVisibility {
                application_id,
                visibility,
            } => write!(
                formatter,
                "application `{application_id}` has invalid persisted visibility `{visibility}`"
            ),
            Self::Persistence { source } => write!(formatter, "exposure store error: {source}"),
        }
    }
}

impl Error for ExposureStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidVisibility { .. } => None,
            Self::Persistence { source } => Some(source),
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

pub fn load_stored_exposure(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<StoredExposure>, ExposureStoreError> {
    let exposure = connection
        .query_row(
            "SELECT desired_visibility, domain, materialization_state
             FROM exposures WHERE application_id = ?1",
            [application_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|source| ExposureStoreError::Persistence { source })?;
    let Some((visibility, domain, materialization_state)) = exposure else {
        return Ok(None);
    };
    let visibility = Visibility::from_database(&visibility).ok_or_else(|| {
        ExposureStoreError::InvalidVisibility {
            application_id: application_id.to_owned(),
            visibility,
        }
    })?;
    Ok(Some(StoredExposure {
        visibility,
        domain,
        materialization_state,
    }))
}

pub fn begin_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &str,
    expected_visibility: Visibility,
    desired_visibility: Visibility,
) -> Result<bool, ApplicationStoreError> {
    let materialization_state = match desired_visibility {
        Visibility::Public => "applying",
        Visibility::Internal => "removing",
    };
    let updated = transaction
        .execute(
            "UPDATE exposures
             SET desired_visibility = ?1,
                 materialization_state = ?2,
                 last_error_code = NULL,
                 last_error_message = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?3 AND desired_visibility = ?4",
            params![
                desired_visibility.database_value(),
                materialization_state,
                application_id,
                expected_visibility.database_value()
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn complete_public_exposure_change(
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

pub fn complete_internal_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<bool, ApplicationStoreError> {
    let updated = transaction
        .execute(
            "UPDATE exposures
             SET active_runtime_id = NULL,
                 materialization_state = 'not_materialized',
                 configuration_version = NULL,
                 last_error_code = NULL,
                 last_error_message = NULL,
                 last_materialized_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1
               AND desired_visibility = 'internal'
               AND materialization_state = 'removing'",
            [application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn record_exposure_change_failure(
    transaction: &Transaction<'_>,
    application_id: &str,
    visibility: Visibility,
    state: &str,
    code: &str,
    message: &str,
) -> Result<bool, ApplicationStoreError> {
    let updated = transaction
        .execute(
            "UPDATE exposures
             SET materialization_state = ?1,
                 last_error_code = ?2,
                 last_error_message = ?3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?4 AND desired_visibility = ?5",
            params![
                state,
                code,
                message,
                application_id,
                visibility.database_value()
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
}

pub fn load_active_runtime_for_exposure(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<(String, String, u16)>, ApplicationStoreError> {
    connection
        .query_row(
            "SELECT ri.id, ri.external_runtime_id, ri.container_port
             FROM runtime_instances ri
             JOIN applications a ON a.active_deployment_id = ri.deployment_id
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
