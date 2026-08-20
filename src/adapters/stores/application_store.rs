use std::error::Error;
use std::fmt;
use std::io;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::domain::application::{
    Application, ApplicationDeploymentSpecification, ApplicationName, ApplicationSource,
    ApplicationSummary, ContainerPort, HealthCheckPath, HealthCheckSpecification,
    HealthCheckStatus, RelativeManifestPath, RepositoryKind, RuntimeSpecification,
};
use crate::domain::delivery::{DeliverySpecification, DeliveryType};
use crate::domain::exposure::{
    ConfirmedRoute, DomainName, Exposure, ExposureConfigurationVersion, ExposureDiagnostic,
    ExposureIntent, ExposureMaterialization, ExposureMaterializationState, Visibility,
};
use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId, SystemId};
use crate::domain::release::OciRepository;
use crate::domain::runtime::DesiredRuntimeState;

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

#[derive(Debug)]
pub enum ExposureStoreError {
    InvalidVisibility {
        application_id: String,
        visibility: String,
    },
    InvalidMaterializationState {
        application_id: String,
        state: String,
    },
    InvalidExposure {
        application_id: String,
        reason: String,
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
            Self::InvalidMaterializationState {
                application_id,
                state,
            } => write!(
                formatter,
                "application `{application_id}` has invalid persisted exposure materialization state `{state}`"
            ),
            Self::InvalidExposure {
                application_id,
                reason,
            } => write!(
                formatter,
                "application `{application_id}` has invalid persisted exposure: {reason}"
            ),
            Self::Persistence { source } => write!(formatter, "exposure store error: {source}"),
        }
    }
}

impl Error for ExposureStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidVisibility { .. }
            | Self::InvalidMaterializationState { .. }
            | Self::InvalidExposure { .. } => None,
            Self::Persistence { source } => Some(source),
        }
    }
}

// Allocates an ID inside the import transaction so related Application records share one boundary.
pub fn generate_id(connection: &Connection) -> Result<String, ApplicationStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|source| ApplicationStoreError::Persistence { source })
}

// Inserts a System when absent without changing the identity of an existing named System.
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

// Resolves the persisted System identity needed by the import transaction.
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
    let desired_runtime_state = DesiredRuntimeState::from_database(&desired_runtime_state)
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
                delivery_type.database_value(),
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
                source.repository_kind().database_value(),
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

// Persists initial visibility intent; route materialization remains unconfirmed.
pub fn insert_exposure(
    transaction: &Transaction<'_>,
    application_id: &str,
    intent: &ExposureIntent,
) -> Result<(), ApplicationStoreError> {
    transaction
        .execute(
            "INSERT INTO exposures (
                application_id, desired_visibility, domain,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                intent.visibility().database_value(),
                intent.domain().map(DomainName::as_str)
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
    let delivery_type = DeliveryType::from_database(&delivery_type).ok_or_else(|| {
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
    let repository_kind = RepositoryKind::from_database(&repository_kind).ok_or_else(|| {
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

// Loads visibility intent and confirmed route state, rejecting invalid persisted enum values.
pub fn load_exposure(
    connection: &Connection,
    application_id: &str,
) -> Result<Option<Exposure>, ExposureStoreError> {
    let exposure = connection
        .query_row(
            "SELECT desired_visibility, domain, active_runtime_id,
                    materialization_state, configuration_version,
                    last_materialized_at, last_error_code, last_error_message
             FROM exposures WHERE application_id = ?1",
            [application_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|source| ExposureStoreError::Persistence { source })?;
    let Some((
        visibility,
        domain,
        active_runtime_id,
        materialization_state,
        configuration_version,
        last_materialized_at,
        last_error_code,
        last_error_message,
    )) = exposure
    else {
        return Ok(None);
    };
    let visibility = Visibility::from_database(&visibility).ok_or_else(|| {
        ExposureStoreError::InvalidVisibility {
            application_id: application_id.to_owned(),
            visibility,
        }
    })?;
    let materialization_state = ExposureMaterializationState::from_database(&materialization_state)
        .ok_or_else(|| ExposureStoreError::InvalidMaterializationState {
            application_id: application_id.to_owned(),
            state: materialization_state,
        })?;
    let domain = domain
        .map(|domain| {
            DomainName::new(&domain).map_err(|error| ExposureStoreError::Persistence {
                source: invalid_text_value(1, "domain", &error.value),
            })
        })
        .transpose()?;
    let intent = ExposureIntent::new(visibility, domain).map_err(|error| {
        ExposureStoreError::InvalidExposure {
            application_id: application_id.to_owned(),
            reason: error.reason,
        }
    })?;
    let confirmed_route = match (
        active_runtime_id,
        configuration_version,
        last_materialized_at,
    ) {
        // Earlier internal-route removal recorded its completion timestamp after
        // clearing the runtime and configuration. It is not confirmed route evidence.
        (None, None, None | Some(_)) => None,
        (Some(runtime_id), Some(configuration_version), Some(materialized_at)) => {
            let configuration_version = ExposureConfigurationVersion::new(&configuration_version)
                .map_err(|error| ExposureStoreError::InvalidExposure {
                application_id: application_id.to_owned(),
                reason: format!("invalid configuration version `{}`", error.value),
            })?;
            Some(
                ConfirmedRoute::new(
                    RuntimeInstanceId::from(runtime_id),
                    configuration_version,
                    materialized_at,
                )
                .map_err(|error| ExposureStoreError::InvalidExposure {
                    application_id: application_id.to_owned(),
                    reason: error.reason,
                })?,
            )
        }
        _ => {
            return Err(ExposureStoreError::InvalidExposure {
                application_id: application_id.to_owned(),
                reason: "confirmed route fields must be all present or all absent".to_owned(),
            });
        }
    };
    let diagnostic = match (last_error_code, last_error_message) {
        (None, None) => None,
        (Some(code), Some(message)) => {
            Some(ExposureDiagnostic::new(&code, &message).map_err(|_| {
                ExposureStoreError::InvalidExposure {
                    application_id: application_id.to_owned(),
                    reason: "diagnostic code and message must be trimmed and non-empty".to_owned(),
                }
            })?)
        }
        _ => {
            return Err(ExposureStoreError::InvalidExposure {
                application_id: application_id.to_owned(),
                reason: "diagnostic code and message must be present together".to_owned(),
            });
        }
    };
    let materialization =
        ExposureMaterialization::hydrate(materialization_state, confirmed_route, diagnostic)
            .map_err(|error| ExposureStoreError::InvalidExposure {
                application_id: application_id.to_owned(),
                reason: error.reason,
            })?;
    Ok(Some(Exposure::new(
        ApplicationId::from(application_id),
        intent,
        materialization,
    )))
}

// Begins a visibility transition with a compare-and-set on the prior intent.
pub fn begin_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &str,
    expected_visibility: Visibility,
    desired_visibility: Visibility,
) -> Result<bool, ApplicationStoreError> {
    let materialization_state = match desired_visibility {
        Visibility::Public => ExposureMaterializationState::Applying,
        Visibility::Internal => ExposureMaterializationState::Removing,
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
                materialization_state.database_value(),
                application_id,
                expected_visibility.database_value()
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
}

// Confirms public route materialization only while the matching transition remains in progress.
pub fn complete_public_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &str,
    runtime_id: &RuntimeInstanceId,
    configuration_version: &ExposureConfigurationVersion,
) -> Result<bool, ApplicationStoreError> {
    let updated = transaction
        .execute(
            "UPDATE exposures
             SET active_runtime_id = ?1,
                 materialization_state = 'active',
                 configuration_version = ?2,
                  last_materialized_at = NULL,
                 last_error_code = NULL,
                 last_error_message = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?3
               AND desired_visibility = 'public'
               AND materialization_state = 'applying'",
            params![
                runtime_id.as_str(),
                configuration_version.as_str(),
                application_id
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
}

// Confirms route removal only while the matching internal transition remains in progress.
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
                  last_materialized_at = NULL,
                  last_error_code = NULL,
                  last_error_message = NULL,
                  updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1
               AND desired_visibility = 'internal'
               AND materialization_state = 'removing'",
            [application_id],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
}

// Records route diagnostics only when the persisted visibility still matches the attempted change.
pub fn record_exposure_change_failure(
    transaction: &Transaction<'_>,
    application_id: &str,
    visibility: Visibility,
    state: ExposureMaterializationState,
    diagnostic: &ExposureDiagnostic,
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
                state.database_value(),
                diagnostic.code(),
                diagnostic.message(),
                application_id,
                visibility.database_value()
            ],
        )
        .map_err(|source| ApplicationStoreError::Persistence { source })?;
    Ok(updated == 1)
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
    let visibility = Visibility::from_database(&visibility).ok_or_else(|| {
        ApplicationStoreError::Persistence {
            source: invalid_text_value(5, "visibility", &visibility),
        }
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
