use rusqlite::{Connection, OptionalExtension, Transaction, params};
use thiserror::Error;

use crate::adapters::stores::PersistenceOutcome;
use crate::adapters::stores::persistence::{
    invalid_text_value, outcome, visibility_from_value, visibility_value,
};
use crate::domain::exposure::{
    ConfirmedRoute, DomainName, Exposure, ExposureConfigurationVersion, ExposureDiagnostic,
    ExposureIntent, ExposureMaterialization, ExposureMaterializationState, Visibility,
};
use crate::domain::identity::{ApplicationId, RuntimeInstanceId};

#[derive(Debug, Error)]
pub enum ExposureStoreError {
    #[error("application `{application_id}` has invalid persisted visibility `{visibility}`")]
    InvalidVisibility {
        application_id: String,
        visibility: String,
    },
    #[error(
        "application `{application_id}` has invalid persisted exposure materialization state `{state}`"
    )]
    InvalidMaterializationState {
        application_id: String,
        state: String,
    },
    #[error("application `{application_id}` has invalid persisted exposure: {reason}")]
    InvalidExposure {
        application_id: String,
        reason: String,
    },
    #[error("exposure store error: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

// Persists initial visibility intent; route materialization remains unconfirmed.
pub(crate) fn insert_exposure(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    intent: &ExposureIntent,
) -> Result<(), ExposureStoreError> {
    transaction
        .execute(
            "INSERT INTO exposures (
                application_id, desired_visibility, domain,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![
                application_id.as_str(),
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
    application_id: &ApplicationId,
) -> Result<Option<Exposure>, ExposureStoreError> {
    let exposure = connection
        .query_row(
            "SELECT desired_visibility, domain, active_runtime_id,
                    materialization_state, configuration_version,
                    last_materialized_at, last_error_code, last_error_message
             FROM exposures WHERE application_id = ?1",
            [application_id.as_str()],
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
            application_id: application_id.to_string(),
            visibility,
        }
    })?;
    let materialization_state = exposure_materialization_state_from_value(&materialization_state)
        .ok_or_else(|| ExposureStoreError::InvalidMaterializationState {
        application_id: application_id.to_string(),
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
            application_id: application_id.to_string(),
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
                application_id: application_id.to_string(),
                reason: format!("invalid configuration version `{}`", error.value),
            })?;
            Some(
                ConfirmedRoute::new(
                    RuntimeInstanceId::from(runtime_id),
                    configuration_version,
                    materialized_at,
                )
                .map_err(|error| ExposureStoreError::InvalidExposure {
                    application_id: application_id.to_string(),
                    reason: error.reason,
                })?,
            )
        }
        _ => {
            return Err(ExposureStoreError::InvalidExposure {
                application_id: application_id.to_string(),
                reason: "confirmed route fields must be all present or all absent".to_owned(),
            });
        }
    };
    let diagnostic = match (last_error_code, last_error_message) {
        (None, None) => None,
        (Some(code), Some(message)) => {
            Some(ExposureDiagnostic::new(&code, &message).map_err(|_| {
                ExposureStoreError::InvalidExposure {
                    application_id: application_id.to_string(),
                    reason: "diagnostic code and message must be trimmed and non-empty".to_owned(),
                }
            })?)
        }
        _ => {
            return Err(ExposureStoreError::InvalidExposure {
                application_id: application_id.to_string(),
                reason: "diagnostic code and message must be present together".to_owned(),
            });
        }
    };
    let materialization =
        ExposureMaterialization::hydrate(materialization_state, confirmed_route, diagnostic)
            .map_err(|error| ExposureStoreError::InvalidExposure {
                application_id: application_id.to_string(),
                reason: error.reason,
            })?;
    Ok(Some(Exposure::new(
        application_id.clone(),
        intent,
        materialization,
    )))
}

// Begins a visibility transition with a compare-and-set on the prior intent.
pub(crate) fn begin_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    expected_visibility: Visibility,
    desired_visibility: Visibility,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let materialization_state = match desired_visibility {
        Visibility::Public => ExposureMaterializationState::Applying,
        Visibility::Internal => ExposureMaterializationState::Removing,
    };
    let updated = transaction.execute("UPDATE exposures SET desired_visibility = ?1, materialization_state = ?2, last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?3 AND desired_visibility = ?4", params![visibility_value(desired_visibility), exposure_materialization_state_value(materialization_state), application_id.as_str(), visibility_value(expected_visibility)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Confirms public route materialization only while the matching transition remains in progress.
pub(crate) fn complete_public_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    runtime_id: &RuntimeInstanceId,
    configuration_version: &ExposureConfigurationVersion,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = transaction.execute("UPDATE exposures SET active_runtime_id = ?1, materialization_state = 'active', configuration_version = ?2, last_materialized_at = CURRENT_TIMESTAMP, last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?3 AND desired_visibility = 'public' AND materialization_state = 'applying'", params![runtime_id.as_str(), configuration_version.as_str(), application_id.as_str()]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Confirms route removal only while the matching internal transition remains in progress.
pub(crate) fn complete_internal_exposure_change(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = transaction.execute("UPDATE exposures SET active_runtime_id = NULL, materialization_state = 'not_materialized', configuration_version = NULL, last_materialized_at = NULL, last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?1 AND desired_visibility = 'internal' AND materialization_state = 'removing'", [application_id.as_str()]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Records route diagnostics only when the persisted visibility still matches the attempted change.
pub(crate) fn record_exposure_change_failure(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    visibility: Visibility,
    state: ExposureMaterializationState,
    diagnostic: &ExposureDiagnostic,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = transaction.execute("UPDATE exposures SET materialization_state = ?1, last_error_code = ?2, last_error_message = ?3, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?4 AND desired_visibility = ?5", params![exposure_materialization_state_value(state), diagnostic.code(), diagnostic.message(), application_id.as_str(), visibility_value(visibility)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Marks a public route as applying before its external materialization begins.
pub(crate) fn begin_public_exposure(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = 'applying', last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?1 AND desired_visibility = 'public'", [application_id.as_str()]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Reserves a known public exposure snapshot for reconciliation before Caddy effects begin.
pub(crate) fn begin_public_exposure_reconciliation(
    connection: &Connection,
    application_id: &ApplicationId,
    expected_state: ExposureMaterializationState,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = 'applying', last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?1 AND desired_visibility = 'public' AND materialization_state = ?2", params![application_id.as_str(), exposure_materialization_state_value(expected_state)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Reserves a known internal exposure snapshot for reconciliation before Caddy effects begin.
pub(crate) fn begin_internal_exposure_reconciliation(
    connection: &Connection,
    application_id: &ApplicationId,
    expected_state: ExposureMaterializationState,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = 'removing', last_error_code = NULL, last_error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?1 AND desired_visibility = 'internal' AND materialization_state = ?2", params![application_id.as_str(), exposure_materialization_state_value(expected_state)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Records reconciliation diagnostics only while its external-effect reservation remains current.
pub(crate) fn record_reconciliation_exposure_failure(
    connection: &Connection,
    application_id: &ApplicationId,
    visibility: Visibility,
    expected_state: ExposureMaterializationState,
    state: ExposureMaterializationState,
    diagnostic: &ExposureDiagnostic,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = ?1, last_error_code = ?2, last_error_message = ?3, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?4 AND desired_visibility = ?5 AND materialization_state = ?6", params![exposure_materialization_state_value(state), diagnostic.code(), diagnostic.message(), application_id.as_str(), visibility_value(visibility), exposure_materialization_state_value(expected_state)]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
}

// Persists the result of public-route compensation without treating a missing row as success.
pub(crate) fn record_public_exposure_failure(
    connection: &Connection,
    application_id: &ApplicationId,
    diagnostic: &ExposureDiagnostic,
    state: ExposureMaterializationState,
) -> Result<PersistenceOutcome, ExposureStoreError> {
    let updated = connection.execute("UPDATE exposures SET materialization_state = ?1, last_error_code = ?2, last_error_message = ?3, updated_at = CURRENT_TIMESTAMP WHERE application_id = ?4 AND desired_visibility = 'public'", params![exposure_materialization_state_value(state), diagnostic.code(), diagnostic.message(), application_id.as_str()]).map_err(|source| ExposureStoreError::Persistence { source })?;
    Ok(outcome(updated))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn connection_with_exposure(visibility: &str, state: &str) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE exposures (
                    application_id TEXT PRIMARY KEY,
                    desired_visibility TEXT NOT NULL,
                    domain TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    active_runtime_id TEXT,
                    materialization_state TEXT NOT NULL,
                    configuration_version TEXT,
                    last_materialized_at TEXT,
                    last_error_code TEXT,
                    last_error_message TEXT);",
            )
            .unwrap();
        connection
            .execute(
                &format!(
                    "INSERT INTO exposures (application_id, desired_visibility, domain, created_at, updated_at, materialization_state)
                     VALUES ('app', '{visibility}', NULL, '2026-01-01', '2026-01-01', '{state}')"
                ),
                [],
            )
            .unwrap();
        connection
    }

    #[test]
    fn reconciliation_reservation_is_stale_unless_the_persisted_snapshot_matches() {
        let connection = connection_with_exposure("internal", "not_materialized");

        assert_eq!(
            begin_internal_exposure_reconciliation(
                &connection,
                &ApplicationId::from("app"),
                ExposureMaterializationState::Active,
            )
            .unwrap(),
            PersistenceOutcome::Stale
        );
        assert_eq!(
            begin_internal_exposure_reconciliation(
                &connection,
                &ApplicationId::from("app"),
                ExposureMaterializationState::NotMaterialized,
            )
            .unwrap(),
            PersistenceOutcome::Updated
        );
        assert_eq!(
            begin_internal_exposure_reconciliation(
                &connection,
                &ApplicationId::from("app"),
                ExposureMaterializationState::NotMaterialized,
            )
            .unwrap(),
            PersistenceOutcome::Stale
        );
    }

    #[test]
    fn internal_completion_requires_the_removing_reservation_and_clears_the_route_triple() {
        let connection = connection_with_exposure("internal", "removing");
        connection
            .execute(
                "UPDATE exposures SET active_runtime_id = 'runtime',
                 configuration_version = 'route bytes\n', last_materialized_at = '2026-01-01'",
                [],
            )
            .unwrap();

        let transaction = connection.unchecked_transaction().unwrap();
        assert_eq!(
            complete_internal_exposure_change(&transaction, &ApplicationId::from("app")).unwrap(),
            PersistenceOutcome::Updated
        );
        transaction.commit().unwrap();

        let (state, runtime): (String, Option<String>) = connection
            .query_row(
                "SELECT materialization_state, active_runtime_id FROM exposures",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "not_materialized");
        assert_eq!(runtime, None);

        let transaction = connection.unchecked_transaction().unwrap();
        assert_eq!(
            complete_internal_exposure_change(&transaction, &ApplicationId::from("app")).unwrap(),
            PersistenceOutcome::Stale
        );
    }

    #[test]
    fn public_completion_requires_the_applying_reservation() {
        let connection = connection_with_exposure("public", "not_materialized");

        let transaction = connection.unchecked_transaction().unwrap();
        assert_eq!(
            complete_public_exposure_change(
                &transaction,
                &ApplicationId::from("app"),
                &RuntimeInstanceId::from("runtime"),
                &ExposureConfigurationVersion::new("route bytes\n").unwrap(),
            )
            .unwrap(),
            PersistenceOutcome::Stale
        );
        drop(transaction);

        connection
            .execute(
                "UPDATE exposures SET materialization_state = 'applying'",
                [],
            )
            .unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        assert_eq!(
            complete_public_exposure_change(
                &transaction,
                &ApplicationId::from("app"),
                &RuntimeInstanceId::from("runtime"),
                &ExposureConfigurationVersion::new("route bytes\n").unwrap(),
            )
            .unwrap(),
            PersistenceOutcome::Updated
        );
    }

    #[test]
    fn failure_recording_is_stale_when_the_expected_reservation_changed() {
        let connection = connection_with_exposure("internal", "not_materialized");
        let diagnostic = ExposureDiagnostic::new("caddy_removal_failed", "reload failed").unwrap();

        assert_eq!(
            record_reconciliation_exposure_failure(
                &connection,
                &ApplicationId::from("app"),
                Visibility::Internal,
                ExposureMaterializationState::Removing,
                ExposureMaterializationState::Diverged,
                &diagnostic,
            )
            .unwrap(),
            PersistenceOutcome::Stale
        );

        begin_internal_exposure_reconciliation(
            &connection,
            &ApplicationId::from("app"),
            ExposureMaterializationState::NotMaterialized,
        )
        .unwrap();
        assert_eq!(
            record_reconciliation_exposure_failure(
                &connection,
                &ApplicationId::from("app"),
                Visibility::Internal,
                ExposureMaterializationState::Removing,
                ExposureMaterializationState::Diverged,
                &diagnostic,
            )
            .unwrap(),
            PersistenceOutcome::Updated
        );
        let (code, state): (String, String) = connection
            .query_row(
                "SELECT last_error_code, materialization_state FROM exposures",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(code, "caddy_removal_failed");
        assert_eq!(state, "diverged");
    }
}
