use std::error::Error;
use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::adapters::caddy_exposure::{
    CaddyRecoveryError, MaterializeCaddyFragmentError, materialize_caddy_fragment,
    remove_caddy_fragment, restore_materialized_caddy_fragment,
};
use crate::adapters::external_health::{ExternalHealthCheckError, check_external_health};
use crate::adapters::local_runtime::{
    ContainerObservation, ObserveContainerError, ObservedRuntimeState, observe_container,
};
use crate::domain::manifest::Visibility;

#[derive(Debug, PartialEq, Eq)]
pub struct ExposureChange {
    pub application_id: String,
    pub visibility: Visibility,
    pub domain: Option<String>,
}

#[derive(Debug)]
pub enum ExposureChangeError {
    ApplicationNotFound {
        application_id: String,
    },
    NoActiveRuntime {
        application_id: String,
    },
    DomainRequired {
        application_id: String,
    },
    InvalidDomain {
        domain: String,
    },
    ObserveFailed {
        source: ObserveContainerError,
    },
    RuntimeNotRunning {
        state: ObservedRuntimeState,
    },
    MissingEndpoint,
    MaterializeFailed {
        source: MaterializeCaddyFragmentError,
    },
    RemoveFragmentFailed {
        source: CaddyRecoveryError,
    },
    ExternalHealthFailed {
        source: ExternalHealthCheckError,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for ExposureChangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::NoActiveRuntime { application_id } => {
                write!(
                    formatter,
                    "application `{application_id}` has no active runtime to expose"
                )
            }
            Self::DomainRequired { application_id } => {
                write!(
                    formatter,
                    "application `{application_id}` requires a domain for public exposure"
                )
            }
            Self::InvalidDomain { domain } => {
                write!(formatter, "domain `{domain}` is not valid")
            }
            Self::ObserveFailed { source } => {
                write!(formatter, "failed to observe runtime: {source}")
            }
            Self::RuntimeNotRunning { state } => {
                write!(formatter, "runtime is not running (state: {state:?})")
            }
            Self::MissingEndpoint => {
                write!(formatter, "runtime has no endpoint available")
            }
            Self::MaterializeFailed { source } => {
                write!(formatter, "failed to materialize Caddy fragment: {source}")
            }
            Self::RemoveFragmentFailed { source } => {
                write!(formatter, "failed to remove Caddy fragment: {source}")
            }
            Self::ExternalHealthFailed { source } => {
                write!(formatter, "external health check failed: {source}")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to persist exposure change: {source}")
            }
        }
    }
}

impl Error for ExposureChangeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ObserveFailed { source } => Some(source),
            Self::MaterializeFailed { source } => Some(source),
            Self::RemoveFragmentFailed { source } => Some(source),
            Self::ExternalHealthFailed { source } => Some(source),
            Self::Persistence { source } => Some(source),
            _ => None,
        }
    }
}

struct ActiveRuntime {
    container_name: String,
    container_port: u16,
    domain: Option<String>,
}

pub fn change_exposure(
    connection: &mut Connection,
    application_id: &str,
    visibility: Visibility,
    managed_directory: &Path,
    caddyfile_path: &Path,
) -> Result<ExposureChange, ExposureChangeError> {
    let application_exists = connection
        .query_row(
            "SELECT COUNT(*) FROM applications WHERE id = ?1",
            [application_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|source| ExposureChangeError::Persistence { source })?;

    if application_exists == 0 {
        return Err(ExposureChangeError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        });
    }

    let current_visibility = connection
        .query_row(
            "SELECT desired_visibility FROM exposures WHERE application_id = ?1",
            [application_id],
            |row| {
                let visibility_text: String = row.get(0)?;
                Ok(Visibility::from_database(&visibility_text).unwrap_or(Visibility::Internal))
            },
        )
        .optional()
        .map_err(|source| ExposureChangeError::Persistence { source })?
        .unwrap_or(Visibility::Internal);

    if current_visibility == visibility {
        let domain = connection
            .query_row(
                "SELECT domain FROM exposures WHERE application_id = ?1",
                [application_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| ExposureChangeError::Persistence { source })?;

        return Ok(ExposureChange {
            application_id: application_id.to_owned(),
            visibility,
            domain,
        });
    }

    match visibility {
        Visibility::Public => make_public(
            connection,
            application_id,
            managed_directory,
            caddyfile_path,
        ),
        Visibility::Internal => make_internal(
            connection,
            application_id,
            managed_directory,
            caddyfile_path,
        ),
    }
}

fn make_public(
    connection: &mut Connection,
    application_id: &str,
    managed_directory: &Path,
    caddyfile_path: &Path,
) -> Result<ExposureChange, ExposureChangeError> {
    let runtime = find_active_runtime(connection, application_id)?;

    let domain = runtime
        .domain
        .ok_or_else(|| ExposureChangeError::DomainRequired {
            application_id: application_id.to_owned(),
        })?;

    let observation = observe_container(&runtime.container_name, runtime.container_port)
        .map_err(|source| ExposureChangeError::ObserveFailed { source })?;

    let endpoint = match observation {
        ContainerObservation {
            state: ObservedRuntimeState::Running,
            endpoint: Some(endpoint),
        } => endpoint,
        ContainerObservation { state, .. } => {
            return Err(ExposureChangeError::RuntimeNotRunning { state });
        }
    };

    let materialized = materialize_caddy_fragment(
        managed_directory,
        caddyfile_path,
        application_id,
        &domain,
        endpoint,
    )
    .map_err(|source| ExposureChangeError::MaterializeFailed { source })?;

    let health_result = check_external_health(&domain, "/", 200);
    if let Err(source) = health_result {
        let _ = restore_materialized_caddy_fragment(&materialized, caddyfile_path);
        return Err(ExposureChangeError::ExternalHealthFailed { source });
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| ExposureChangeError::Persistence { source })?;

    transaction
        .execute(
            "UPDATE exposures
             SET desired_visibility = 'public',
                 domain = ?1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?2",
            rusqlite::params![domain, application_id],
        )
        .map_err(|source| ExposureChangeError::Persistence { source })?;

    transaction
        .commit()
        .map_err(|source| ExposureChangeError::Persistence { source })?;

    Ok(ExposureChange {
        application_id: application_id.to_owned(),
        visibility: Visibility::Public,
        domain: Some(domain),
    })
}

fn make_internal(
    connection: &mut Connection,
    application_id: &str,
    managed_directory: &Path,
    caddyfile_path: &Path,
) -> Result<ExposureChange, ExposureChangeError> {
    let fragment_path = managed_directory.join(format!("{application_id}.caddy"));
    if fragment_path.exists() {
        remove_caddy_fragment(managed_directory, application_id, caddyfile_path)
            .map_err(|source| ExposureChangeError::RemoveFragmentFailed { source })?;
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| ExposureChangeError::Persistence { source })?;

    transaction
        .execute(
            "UPDATE exposures
             SET desired_visibility = 'internal',
                 updated_at = CURRENT_TIMESTAMP
             WHERE application_id = ?1",
            [application_id],
        )
        .map_err(|source| ExposureChangeError::Persistence { source })?;

    transaction
        .commit()
        .map_err(|source| ExposureChangeError::Persistence { source })?;

    let domain = connection
        .query_row(
            "SELECT domain FROM exposures WHERE application_id = ?1",
            [application_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| ExposureChangeError::Persistence { source })?;

    Ok(ExposureChange {
        application_id: application_id.to_owned(),
        visibility: Visibility::Internal,
        domain,
    })
}

fn find_active_runtime(
    connection: &Connection,
    application_id: &str,
) -> Result<ActiveRuntime, ExposureChangeError> {
    connection
        .query_row(
            "SELECT ri.external_runtime_id, ri.container_port, e.domain
              FROM runtime_instances ri
              JOIN applications a ON a.active_deployment_id = ri.deployment_id
              JOIN exposures e ON e.application_id = ri.application_id
              WHERE a.id = ?1
                AND ri.state = 'running'
                AND ri.removed_at IS NULL",
            [application_id],
            |row| {
                Ok(ActiveRuntime {
                    container_name: row.get(0)?,
                    container_port: row.get(1)?,
                    domain: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|source| ExposureChangeError::Persistence { source })?
        .ok_or_else(|| ExposureChangeError::NoActiveRuntime {
            application_id: application_id.to_owned(),
        })
}
