use std::error::Error;
use std::fmt;
use std::io;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::adapters::stores::PersistenceOutcome;
use crate::domain::exposure::{
    ConfirmedRoute, DomainName, Exposure, ExposureConfigurationVersion, ExposureDiagnostic,
    ExposureIntent, ExposureMaterialization, ExposureMaterializationState, Visibility,
};
use crate::domain::identity::{ApplicationId, RuntimeInstanceId};

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

// Persists initial visibility intent; route materialization remains unconfirmed.
pub fn insert_exposure(
    transaction: &Transaction<'_>,
    application_id: &str,
    intent: &ExposureIntent,
) -> Result<(), ExposureStoreError> {
    transaction
        .execute(
            "INSERT INTO exposures (
                application_id, desired_visibility, domain,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id,
                visibility_value(intent.visibility()),
                intent.domain().map(DomainName::as_str)
            ],
        )
        .map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(())
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
    let visibility = visibility_from_value(&visibility).ok_or_else(|| {
        ExposureStoreError::InvalidVisibility {
            application_id: application_id.to_owned(),
            visibility,
        }
    })?;
    let materialization_state = exposure_materialization_state_from_value(&materialization_state)
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
        // Earlier internal-route removal recorded its completion timestamp after clearing the runtime and configuration.
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
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let materialization_state = match desired_visibility {
        Visibility::Public => ExposureMaterializationState::Applying,
        Visibility::Internal => ExposureMaterializationState::Removing,
    };
    let updated = transaction.execute("UPDATE exposures SET desired_visibility = ?1, materialization_state = ?2, last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?3 AND desired_visibility = ?4", params![visibility_value(desired_visibility), exposure_materialization_state_value(materialization_state), application_id, visibility_value(expected_visibility)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Confirms public route materialization only while the matching transition remains in progress.
pub fn complete_public_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &str,
    runtime_id: &RuntimeInstanceId,
    configuration_version: &ExposureConfigurationVersion,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = transaction.execute("UPDATE exposures SET active_runtime_id = ?1, materialization_state = 'active', configuration_version = ?2, last_materialized_at = CURRENT_TIMESTAMP, last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?3 AND desired_visibility = 'public' AND materialization_state = 'applying'", params![runtime_id.as_str(), configuration_version.as_str(), application_id]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Confirms route removal only while the matching internal transition remains in progress.
pub fn complete_internal_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &str,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = transaction.execute("UPDATE exposures SET active_runtime_id = NULL, materialization_state = 'not_materialized', configuration_version = NULL, last_materialized_at = NULL, last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?1 AND desired_visibility = 'internal' AND materialization_state = 'removing'", [application_id]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Records route diagnostics only when the persisted visibility still matches the attempted change.
pub fn record_exposure_change_failure(
    transaction: &Transaction<'_>,
    application_id: &str,
    visibility: Visibility,
    state: ExposureMaterializationState,
    diagnostic: &ExposureDiagnostic,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = transaction.execute("UPDATE exposures SET materialization_state = ?1, last_error_code = ?2, last_error_message = ?3, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?4 AND desired_visibility = ?5", params![exposure_materialization_state_value(state), diagnostic.code(), diagnostic.message(), application_id, visibility_value(visibility)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Marks a public route as applying before its external materialization begins.
pub fn begin_public_exposure(
    connection: &Connection,
    application_id: &str,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = 'applying', last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?1 AND desired_visibility = 'public'", [application_id]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Reserves a known public exposure snapshot for reconciliation before Caddy effects begin.
pub fn begin_public_exposure_reconciliation(
    connection: &Connection,
    application_id: &str,
    expected_state: ExposureMaterializationState,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = 'applying', last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?1 AND desired_visibility = 'public' AND materialization_state = ?2", params![application_id, exposure_materialization_state_value(expected_state)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Reserves a known internal exposure snapshot for reconciliation before Caddy effects begin.
pub fn begin_internal_exposure_reconciliation(
    connection: &Connection,
    application_id: &str,
    expected_state: ExposureMaterializationState,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = 'removing', last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?1 AND desired_visibility = 'internal' AND materialization_state = ?2", params![application_id, exposure_materialization_state_value(expected_state)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Records reconciliation diagnostics only while its external-effect reservation remains current.
pub fn record_reconciliation_exposure_failure(
    connection: &Connection,
    application_id: &str,
    visibility: Visibility,
    expected_state: ExposureMaterializationState,
    state: ExposureMaterializationState,
    diagnostic: &ExposureDiagnostic,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = ?1, last_error_code = ?2, last_error_message = ?3, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?4 AND desired_visibility = ?5 AND materialization_state = ?6", params![exposure_materialization_state_value(state), diagnostic.code(), diagnostic.message(), application_id, visibility_value(visibility), exposure_materialization_state_value(expected_state)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Persists the result of public-route compensation without treating a missing row as success.
pub fn record_public_exposure_failure(
    connection: &Connection,
    application_id: &str,
    diagnostic: &ExposureDiagnostic,
    state: ExposureMaterializationState,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = ?1, last_error_code = ?2, last_error_message = ?3, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?4 AND desired_visibility = 'public'", params![exposure_materialization_state_value(state), diagnostic.code(), diagnostic.message(), application_id]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

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

fn visibility_value(value: Visibility) -> &'static str {
    match value {
        Visibility::Internal => "internal",
        Visibility::Public => "public",
    }
}

fn visibility_from_value(value: &str) -> Option<Visibility> {
    match value {
        "internal" => Some(Visibility::Internal),
        "public" => Some(Visibility::Public),
        _ => None,
    }
}

fn exposure_materialization_state_value(value: ExposureMaterializationState) -> &'static str {
    match value {
        ExposureMaterializationState::NotMaterialized => "not_materialized",
        ExposureMaterializationState::Applying => "applying",
        ExposureMaterializationState::Active => "active",
        ExposureMaterializationState::Removing => "removing",
        ExposureMaterializationState::Failed => "failed",
        ExposureMaterializationState::Diverged => "diverged",
    }
}

fn exposure_materialization_state_from_value(value: &str) -> Option<ExposureMaterializationState> {
    match value {
        "not_materialized" => Some(ExposureMaterializationState::NotMaterialized),
        "applying" => Some(ExposureMaterializationState::Applying),
        "active" => Some(ExposureMaterializationState::Active),
        "removing" => Some(ExposureMaterializationState::Removing),
        "failed" => Some(ExposureMaterializationState::Failed),
        "diverged" => Some(ExposureMaterializationState::Diverged),
        _ => None,
    }
}
