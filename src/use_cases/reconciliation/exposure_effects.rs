use rusqlite::Connection;

use crate::adapters::caddy_exposure::{
    CaddyRecoveryError, MaterializeCaddyFragmentError, MaterializedCaddyFragment,
    canonical_fragment_contents, materialize_caddy_fragment, remove_caddy_fragment,
    restore_materialized_caddy_fragment, restore_removed_caddy_fragment,
};
use crate::adapters::health_check_external::check_external_health;
use crate::adapters::stores::{PersistenceOutcome, exposure_store};
use crate::domain::deployment::DeploymentFailureCode;
use crate::domain::exposure::{
    DomainName, ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent,
    ExposureMaterializationState, ExposureOutcome, Visibility,
};
use crate::domain::identity::ApplicationId;
use crate::domain::reconciliation::{PublicExposureFailure, ReconciliationInput};
use crate::domain::runtime::RuntimeInstance;

use super::load::persistence_error;
use super::{
    ReconciliationReadError, ReconciliationResult, inconsistent_input, required_active_runtime,
};

// Removes a stale managed route from an internally exposed application after CAS reservation.
pub(crate) fn remove_internal_route(
    connection: &mut Connection,
    input: &ReconciliationInput,
    expected_state: ExposureMaterializationState,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    reserve_exposure(
        connection,
        &input.desired.application.id,
        Visibility::Internal,
        expected_state,
    )?;
    let removed = match remove_caddy_fragment(
        managed_caddy_directory,
        &input.desired.application.id,
        caddyfile_path,
    ) {
        Ok(removed) => removed,
        Err(source) => {
            return record_exposure_failure(
                connection,
                &input.desired.application.id,
                Visibility::Internal,
                ExposureMaterializationState::Removing,
                "caddy_removal_failed",
                &source.to_string(),
                recovery_outcome(source.recovery_failed()),
            );
        }
    };
    let transaction = connection.transaction().map_err(persistence_error)?;
    let completed = exposure_store::complete_internal_exposure_change(
        &transaction,
        &input.desired.application.id,
    )
    .map_err(|source| ReconciliationReadError::Exposure { source })?;
    if completed == PersistenceOutcome::Stale {
        drop(transaction);
        let outcome = restoration_outcome(restore_removed_caddy_fragment(&removed, caddyfile_path));
        return record_exposure_failure(
            connection,
            &input.desired.application.id,
            Visibility::Internal,
            ExposureMaterializationState::Removing,
            "exposure_changed",
            "exposure changed while Caddy route removal was being confirmed",
            outcome,
        );
    }
    transaction.commit().map_err(persistence_error)?;
    Ok(ReconciliationResult::ExposureRepaired)
}

// Materializes the canonical public route, verifies external health, then persists confirmation.
//
// Stages: prepare and reserve the exposure snapshot, apply the canonical Caddy
// fragment, verify the route through its public domain, then confirm the change
// under CAS. Every compensated phase restores the prior route before recording
// its diagnostic.
pub(crate) fn materialize_public_route(
    connection: &mut Connection,
    input: &ReconciliationInput,
    expected_state: ExposureMaterializationState,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let (domain, runtime, configuration_version) = prepare_public_route(input)?;
    reserve_exposure(
        connection,
        &input.desired.application.id,
        Visibility::Public,
        expected_state,
    )?;

    let materialized = match materialize_caddy_fragment(
        managed_caddy_directory,
        caddyfile_path,
        &input.desired.application.id,
        domain,
        runtime.expected_endpoint,
    ) {
        Ok(materialized) => materialized,
        Err(source) => return record_materialization_failure(connection, input, &source),
    };

    if let Some(result) =
        verify_public_route_or_rollback(connection, input, domain, &materialized, caddyfile_path)?
    {
        return Ok(result);
    }
    confirm_public_route_or_rollback(
        connection,
        input,
        runtime,
        configuration_version,
        &materialized,
        caddyfile_path,
    )
}

// Validates the desired public intent, resolves the serving runtime, and derives the
// canonical configuration version that confirmation must persist.
fn prepare_public_route(
    input: &ReconciliationInput,
) -> Result<(&DomainName, &RuntimeInstance, ExposureConfigurationVersion), ReconciliationReadError>
{
    let Some(exposure) = &input.desired.exposure else {
        return Err(inconsistent_input(
            "public route materialization requires an exposure",
        ));
    };
    let ExposureIntent::Public { domain } = exposure.intent() else {
        return Err(inconsistent_input(
            "public route materialization requires public intent",
        ));
    };
    let (_, runtime) = required_active_runtime(input)?;
    let contents = canonical_fragment_contents(domain, runtime.expected_endpoint);
    let configuration_version = ExposureConfigurationVersion::new(&contents).map_err(|source| {
        ReconciliationReadError::NotConverged {
            reason: source.to_string(),
        }
    })?;
    Ok((domain, runtime, configuration_version))
}

// Translates rejected fragment materialization into its recorded failure result; an
// unsuccessful adapter recovery upgrades the outcome to divergence.
fn record_materialization_failure(
    connection: &Connection,
    input: &ReconciliationInput,
    source: &MaterializeCaddyFragmentError,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    record_exposure_failure(
        connection,
        &input.desired.application.id,
        Visibility::Public,
        ExposureMaterializationState::Applying,
        DeploymentFailureCode::CaddyMaterialization.as_str(),
        &source.to_string(),
        recovery_outcome(source.recovery_failed()),
    )
}

// Verifies the applied route through its public domain. A rejection restores the
// prior route and records the diagnostic, terminating reconciliation with that
// result; an accepted route yields `None` so confirmation may proceed.
fn verify_public_route_or_rollback(
    connection: &Connection,
    input: &ReconciliationInput,
    domain: &DomainName,
    materialized: &MaterializedCaddyFragment,
    caddyfile_path: &std::path::Path,
) -> Result<Option<ReconciliationResult>, ReconciliationReadError> {
    let Some(specification) = input.persisted.specification.as_ref() else {
        return Err(ReconciliationReadError::NotConverged {
            reason: "application has no deployment specification for public health".to_owned(),
        });
    };
    let health_check = specification.runtime.health_check();
    if let Err(source) =
        check_external_health(domain, health_check.path(), health_check.expected_status())
    {
        let outcome = restoration_outcome(restore_materialized_caddy_fragment(
            materialized,
            caddyfile_path,
        ));
        return record_exposure_failure(
            connection,
            &input.desired.application.id,
            Visibility::Public,
            ExposureMaterializationState::Applying,
            DeploymentFailureCode::ExternalHealthCheck.as_str(),
            &source.to_string(),
            outcome,
        )
        .map(Some);
    }
    Ok(None)
}

// Confirms the externally verified route under CAS. A lost reservation restores the
// prior route and records the diagnostic before surfacing the concurrency outcome.
fn confirm_public_route_or_rollback(
    connection: &mut Connection,
    input: &ReconciliationInput,
    runtime: &RuntimeInstance,
    configuration_version: ExposureConfigurationVersion,
    materialized: &MaterializedCaddyFragment,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let transaction = connection.transaction().map_err(persistence_error)?;
    let completed = exposure_store::complete_public_exposure_change(
        &transaction,
        &input.desired.application.id,
        &runtime.id,
        &configuration_version,
    )
    .map_err(|source| ReconciliationReadError::Exposure { source })?;
    if completed == PersistenceOutcome::Stale {
        drop(transaction);
        let outcome = restoration_outcome(restore_materialized_caddy_fragment(
            materialized,
            caddyfile_path,
        ));
        return record_exposure_failure(
            connection,
            &input.desired.application.id,
            Visibility::Public,
            ExposureMaterializationState::Applying,
            "exposure_changed",
            "exposure changed while Caddy route materialization was being confirmed",
            outcome,
        );
    }
    transaction.commit().map_err(persistence_error)?;
    Ok(ReconciliationResult::ExposureRepaired)
}

pub(crate) fn record_public_exposure_failure(
    connection: &Connection,
    input: &ReconciliationInput,
    failure: &PublicExposureFailure,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    record_exposure_failure(
        connection,
        &input.desired.application.id,
        Visibility::Public,
        failure.expected_state,
        failure.kind.code(),
        failure.kind.message(),
        ExposureOutcome::Failed,
    )
}

fn reserve_exposure(
    connection: &Connection,
    application_id: &ApplicationId,
    visibility: Visibility,
    state: ExposureMaterializationState,
) -> Result<(), ReconciliationReadError> {
    let outcome = match visibility {
        Visibility::Public => {
            exposure_store::begin_public_exposure_reconciliation(connection, application_id, state)
        }
        Visibility::Internal => exposure_store::begin_internal_exposure_reconciliation(
            connection,
            application_id,
            state,
        ),
    }
    .map_err(|source| ReconciliationReadError::Exposure { source })?;
    if outcome == PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: "exposure changed before reconciliation could reserve it".to_owned(),
        });
    }
    Ok(())
}

// A failed adapter recovery leaves host state outside every persisted outcome;
// a recovered one ends the change as a plain recorded failure.
fn recovery_outcome(recovery_failed: bool) -> ExposureOutcome {
    if recovery_failed {
        ExposureOutcome::Diverged
    } else {
        ExposureOutcome::Failed
    }
}

// A compensating restore either reinstates the recorded prior route (a clean
// failure) or leaves host state no persisted outcome can describe (divergence).
fn restoration_outcome(restored: Result<(), CaddyRecoveryError>) -> ExposureOutcome {
    match restored {
        Ok(()) => ExposureOutcome::Failed,
        Err(_) => ExposureOutcome::Diverged,
    }
}

// Persists why an exposure change ended abnormally and returns the matching
// terminal reconciliation result; a lost reservation refuses to converge.
fn record_exposure_failure(
    connection: &Connection,
    application_id: &ApplicationId,
    visibility: Visibility,
    expected_state: ExposureMaterializationState,
    code: &str,
    message: &str,
    outcome: ExposureOutcome,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let diagnostic = ExposureDiagnostic::new(code, message).map_err(|_| {
        ReconciliationReadError::NotConverged {
            reason: "reconciliation produced an invalid exposure diagnostic".to_owned(),
        }
    })?;
    let recorded = exposure_store::record_reconciliation_exposure_failure(
        connection,
        application_id,
        visibility,
        expected_state,
        outcome.state(),
        &diagnostic,
    )
    .map_err(|source| ReconciliationReadError::Exposure { source })?;
    if recorded == PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: "exposure changed before reconciliation failure could be recorded".to_owned(),
        });
    }
    Ok(match outcome {
        ExposureOutcome::Diverged => ReconciliationResult::Diverged {
            reason: message.to_owned(),
        },
        ExposureOutcome::Failed => ReconciliationResult::Failed {
            reason: message.to_owned(),
        },
    })
}
