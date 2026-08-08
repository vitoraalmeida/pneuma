use std::error::Error;
use std::fmt;
use std::net::SocketAddr;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::adapters::health_check::{HealthCheckError, check_internal_health};
use crate::adapters::local_runtime::{
    ContainerObservation, ControlContainerError, ObserveContainerError, ObservedRuntimeState,
    observe_container, start_container,
};

#[derive(Debug, PartialEq, Eq)]
pub struct RolledBackDeployment {
    pub deployment_id: String,
    pub runtime_id: String,
    pub commit_sha: String,
    pub finished_at: String,
}

#[derive(Debug)]
pub enum RollbackError {
    ApplicationNotFound {
        application_id: String,
    },
    NoPreviousDeployment {
        application_id: String,
    },
    PreviousRuntimeMissing {
        runtime_id: String,
    },
    ImageNotFound {
        image: String,
    },
    StartFailed {
        runtime_id: String,
        source: ControlContainerError,
    },
    ObserveFailed {
        runtime_id: String,
        source: ObserveContainerError,
    },
    HealthCheckFailed {
        runtime_id: String,
        source: HealthCheckError,
    },
    PromotionFailed {
        source: rusqlite::Error,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for RollbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::NoPreviousDeployment { application_id } => {
                write!(
                    formatter,
                    "application `{application_id}` has no previous successful deployment to roll back to"
                )
            }
            Self::PreviousRuntimeMissing { runtime_id } => {
                write!(
                    formatter,
                    "previous runtime `{runtime_id}` is missing and cannot be rolled back to"
                )
            }
            Self::ImageNotFound { image } => {
                write!(formatter, "image `{image}` is not available locally")
            }
            Self::StartFailed { runtime_id, source } => {
                write!(
                    formatter,
                    "failed to start runtime `{runtime_id}`: {source}"
                )
            }
            Self::ObserveFailed { runtime_id, source } => {
                write!(
                    formatter,
                    "failed to observe runtime `{runtime_id}`: {source}"
                )
            }
            Self::HealthCheckFailed { runtime_id, source } => {
                write!(
                    formatter,
                    "health check failed for runtime `{runtime_id}`: {source}"
                )
            }
            Self::PromotionFailed { source } => {
                write!(formatter, "failed to promote previous runtime: {source}")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to record rollback: {source}")
            }
        }
    }
}

impl Error for RollbackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::StartFailed { source, .. } => Some(source),
            Self::ObserveFailed { source, .. } => Some(source),
            Self::HealthCheckFailed { source, .. } => Some(source),
            Self::PromotionFailed { source } => Some(source),
            Self::Persistence { source } => Some(source),
            _ => None,
        }
    }
}

struct PreviousDeployment {
    deployment_id: String,
    commit_sha: String,
    runtime_id: String,
    container_name: String,
    container_port: u16,
    health_path: String,
    expected_status: u16,
    finished_at: String,
}

pub fn rollback_deployment(
    connection: &mut Connection,
    application_id: &str,
) -> Result<RolledBackDeployment, RollbackError> {
    let application_exists = connection
        .query_row(
            "SELECT COUNT(*) FROM applications WHERE id = ?1",
            [application_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|source| RollbackError::Persistence { source })?;

    if application_exists == 0 {
        return Err(RollbackError::ApplicationNotFound {
            application_id: application_id.to_owned(),
        });
    }

    let previous = find_previous_successful_deployment(connection, application_id)?;

    let observation = observe_container(&previous.container_name, previous.container_port)
        .map_err(|source| RollbackError::ObserveFailed {
            runtime_id: previous.runtime_id.clone(),
            source,
        })?;

    let endpoint = ensure_runtime_running(&previous, &observation)?;

    let health = check_internal_health(endpoint, &previous.health_path, previous.expected_status)
        .map_err(|source| RollbackError::HealthCheckFailed {
        runtime_id: previous.runtime_id.clone(),
        source,
    })?;

    if !matches!(
        health,
        crate::adapters::health_check::HealthCheckResult::Healthy { .. }
    ) {
        return Err(RollbackError::HealthCheckFailed {
            runtime_id: previous.runtime_id.clone(),
            source: HealthCheckError::InvalidPath,
        });
    }

    promote_previous_to_current(connection, application_id, &previous)?;

    Ok(RolledBackDeployment {
        deployment_id: previous.deployment_id,
        runtime_id: previous.runtime_id,
        commit_sha: previous.commit_sha,
        finished_at: previous.finished_at,
    })
}

fn find_previous_successful_deployment(
    connection: &Connection,
    application_id: &str,
) -> Result<PreviousDeployment, RollbackError> {
    connection
        .query_row(
            "SELECT d.id, r.commit_sha,
                    ri.id, ri.external_runtime_id, ri.container_port,
                    hcs.path, hcs.expected_status, d.finished_at
             FROM deployments d
             JOIN revisions r ON r.id = d.revision_id
             JOIN runtime_instances ri ON ri.deployment_id = d.id
             JOIN health_check_specs hcs ON hcs.application_id = d.application_id
             WHERE d.application_id = ?1
               AND d.status = 'succeeded'
               AND d.finished_at IS NOT NULL
               AND ri.role IN ('current', 'previous')
               AND ri.removed_at IS NULL
             ORDER BY d.finished_at DESC
             LIMIT 1",
            [application_id],
            |row| {
                Ok(PreviousDeployment {
                    deployment_id: row.get(0)?,
                    commit_sha: row.get(1)?,
                    runtime_id: row.get(2)?,
                    container_name: row.get(3)?,
                    container_port: row.get(4)?,
                    health_path: row.get(5)?,
                    expected_status: row.get(6)?,
                    finished_at: row.get::<_, String>(7)?,
                })
            },
        )
        .optional()
        .map_err(|source| RollbackError::Persistence { source })?
        .ok_or_else(|| RollbackError::NoPreviousDeployment {
            application_id: application_id.to_owned(),
        })
}

fn ensure_runtime_running(
    previous: &PreviousDeployment,
    observation: &ContainerObservation,
) -> Result<SocketAddr, RollbackError> {
    match observation.state {
        ObservedRuntimeState::Running => {
            observation
                .endpoint
                .ok_or_else(|| RollbackError::PreviousRuntimeMissing {
                    runtime_id: previous.runtime_id.clone(),
                })
        }
        ObservedRuntimeState::Created | ObservedRuntimeState::Stopped => {
            start_container(&previous.container_name).map_err(|source| {
                RollbackError::StartFailed {
                    runtime_id: previous.runtime_id.clone(),
                    source,
                }
            })?;
            let new_observation =
                observe_container(&previous.container_name, previous.container_port).map_err(
                    |source| RollbackError::ObserveFailed {
                        runtime_id: previous.runtime_id.clone(),
                        source,
                    },
                )?;
            match new_observation {
                ContainerObservation {
                    state: ObservedRuntimeState::Running,
                    endpoint: Some(endpoint),
                } => Ok(endpoint),
                _ => Err(RollbackError::PreviousRuntimeMissing {
                    runtime_id: previous.runtime_id.clone(),
                }),
            }
        }
        _ => Err(RollbackError::PreviousRuntimeMissing {
            runtime_id: previous.runtime_id.clone(),
        }),
    }
}

fn promote_previous_to_current(
    connection: &mut Connection,
    application_id: &str,
    previous: &PreviousDeployment,
) -> Result<(), RollbackError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| RollbackError::PromotionFailed { source })?;

    let current_runtime_id = transaction
        .query_row(
            "SELECT id FROM runtime_instances
             WHERE application_id = ?1
               AND role = 'current'
               AND removed_at IS NULL",
            [application_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|source| RollbackError::PromotionFailed { source })?;

    if let Some(current_runtime_id) = current_runtime_id {
        transaction
            .execute(
                "UPDATE runtime_instances
                 SET role = 'previous', updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND role = 'current' AND removed_at IS NULL",
                [current_runtime_id],
            )
            .map_err(|source| RollbackError::PromotionFailed { source })?;
    }

    transaction
        .execute(
            "UPDATE runtime_instances
             SET role = 'current', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND role = 'previous' AND removed_at IS NULL",
            [&previous.runtime_id],
        )
        .map_err(|source| RollbackError::PromotionFailed { source })?;

    transaction
        .commit()
        .map_err(|source| RollbackError::PromotionFailed { source })?;

    Ok(())
}
