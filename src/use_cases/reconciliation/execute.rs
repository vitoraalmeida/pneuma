use rusqlite::Connection;

use crate::adapters::caddy_exposure::{
    canonical_fragment_contents, materialize_caddy_fragment, remove_caddy_fragment,
    restore_materialized_caddy_fragment, restore_removed_caddy_fragment,
};
use crate::adapters::health_check_external::check_external_health;
use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::adapters::local_runtime::observe_named_container;
use crate::adapters::stores::{PersistenceOutcome, exposure_store, runtime_store};
use crate::adapters::systemd_quadlet::{
    container_name, daemon_reload, start, unit_name, write_unit,
};
use crate::domain::exposure::{
    ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent, ExposureMaterializationState,
    Visibility,
};
use crate::domain::identity::ApplicationId;
use crate::domain::reconciliation::{
    NamedContainerObservation, PublicExposureFailure, ReconciliationDecision,
    ReconciliationDecisionError, ReconciliationExpectations, ReconciliationInput,
    RuntimeIdentityRepair, RuntimeRematerialization,
};
use crate::domain::runtime::ObservedRuntimeState;

use super::load::persistence_error;
use super::{
    ReconciliationReadError, ReconciliationResult, host_port, inconsistent_input,
    required_active_runtime,
};

// Translates a pure decision refusal into the read error surface without changing its message.
pub(crate) fn reconciliation_decision_reason(error: ReconciliationDecisionError) -> String {
    match error {
        ReconciliationDecisionError::UnhandledDrift => {
            "drift has no automatic repair; manual intervention is required".to_owned()
        }
        ReconciliationDecisionError::InvalidRouteFragment(source) => source.to_string(),
    }
}

// Executes one decided action; every effect below corresponds to exactly one decision variant.
pub(crate) fn execute_reconciliation_decision(
    connection: &mut Connection,
    input: &ReconciliationInput,
    expectations: &ReconciliationExpectations,
    decision: ReconciliationDecision,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    match decision {
        ReconciliationDecision::InSync => Ok(ReconciliationResult::NoOp),
        ReconciliationDecision::RepairRuntime(repair) => {
            confirm_runtime_identity(connection, input, &repair)
        }
        ReconciliationDecision::RematerializeRuntime(plan) => {
            rematerialize_runtime(connection, input, expectations, plan)
        }
        ReconciliationDecision::RemoveInternalRoute { expected_state } => remove_internal_route(
            connection,
            input,
            expected_state,
            managed_caddy_directory,
            caddyfile_path,
        ),
        ReconciliationDecision::MaterializePublicRoute { expected_state } => {
            materialize_public_route(
                connection,
                input,
                expected_state,
                managed_caddy_directory,
                caddyfile_path,
            )
        }
        ReconciliationDecision::RecordPublicExposureFailure(failure) => {
            record_public_exposure_failure(connection, input, &failure)
        }
        ReconciliationDecision::RequireManualIntervention(reason) => {
            Ok(ReconciliationResult::ManualIntervention { reason })
        }
    }
}

// Confirms a proven recreated container by swapping the recorded container id under CAS.
fn confirm_runtime_identity(
    connection: &Connection,
    input: &ReconciliationInput,
    repair: &RuntimeIdentityRepair,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let (_active, runtime) = required_active_runtime(input)?;
    let outcome = runtime_store::reconcile_external_runtime_id(
        connection,
        &runtime.id,
        &runtime.external_runtime_id,
        &repair.container_id,
    )
    .map_err(|source| ReconciliationReadError::Runtime { source })?;
    if outcome == PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: format!(
                "runtime `{}` changed before identity reconciliation",
                runtime.id
            ),
        });
    }
    Ok(ReconciliationResult::Repaired {
        runtime_id: repair.runtime_id.to_string(),
        container_id: repair.container_id.to_string(),
    })
}

// Rematerializes the decided runtime from persisted identity and confirms it before persisting.
fn rematerialize_runtime(
    connection: &Connection,
    input: &ReconciliationInput,
    expectations: &ReconciliationExpectations,
    plan: RuntimeRematerialization,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let application = &input.desired.application;
    let (active, runtime) = required_active_runtime(input)?;
    let specification = input.persisted.specification.as_ref().ok_or_else(|| {
        ReconciliationReadError::NotConverged {
            reason: "application has no persisted deployment specification".to_owned(),
        }
    })?;
    let unit = unit_name(&application.name, &active.deployment.id);
    if plan.unit_needs_write {
        write_unit(
            &application.name,
            &active.deployment.id,
            &active.release.artifact,
            runtime.container_port,
            host_port(runtime)?,
        )
        .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
        daemon_reload().map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    }
    start(&unit).map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    let NamedContainerObservation::Present {
        id,
        name,
        image_reference,
        application_label,
        image_digest_label,
        observation: container_observation,
    } = observe_named_container(
        &container_name(&application.name, &active.deployment.id),
        runtime.container_port,
    )
    .map_err(|source| ReconciliationReadError::ObserveNamedContainer { source })?
    else {
        return Ok(ReconciliationResult::Failed {
            reason: "rematerialized Quadlet did not create its expected container".to_owned(),
        });
    };
    if *container_observation.state() != ObservedRuntimeState::Running
        || name.trim_start_matches('/') != expectations.container_name
        || image_reference != active.release.artifact.reference()
        || application_label.as_deref() != Some(application.name.as_str())
        || image_digest_label.as_deref() != Some(active.release.artifact.digest())
        || container_observation.observed_endpoint()
            != Some(runtime.expected_endpoint.socket_addr())
    {
        return Ok(ReconciliationResult::ManualIntervention {
            reason: "rematerialized container identity or endpoint differs from persisted intent"
                .to_owned(),
        });
    }
    match check_internal_health(
        runtime.expected_endpoint.socket_addr(),
        specification.runtime.health_check(),
    )
    .map_err(|source| ReconciliationReadError::NotConverged {
        reason: source.to_string(),
    })? {
        HealthCheckResult::Healthy { .. } => {}
        HealthCheckResult::Unhealthy { failure, .. } => {
            return Ok(ReconciliationResult::Failed {
                reason: format!(
                    "rematerialized runtime failed its internal health check: {failure:?}"
                ),
            });
        }
    }
    let outcome = runtime_store::reconcile_external_runtime_id(
        connection,
        &runtime.id,
        &runtime.external_runtime_id,
        &id,
    )
    .map_err(|source| ReconciliationReadError::Runtime { source })?;
    if outcome == PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: format!(
                "runtime `{}` changed before rematerialization could be confirmed",
                runtime.id
            ),
        });
    }
    Ok(ReconciliationResult::Repaired {
        runtime_id: runtime.id.to_string(),
        container_id: id.to_string(),
    })
}

// Removes a stale managed route from an internally exposed application after CAS reservation.
fn remove_internal_route(
    connection: &mut Connection,
    input: &ReconciliationInput,
    expected_state: ExposureMaterializationState,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    reserve_exposure(
        connection,
        input.desired.application.id.as_str(),
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
                input.desired.application.id.as_str(),
                Visibility::Internal,
                ExposureMaterializationState::Removing,
                "caddy_removal_failed",
                &source.to_string(),
                source.recovery_failed(),
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
        let recovery_failed = restore_removed_caddy_fragment(&removed, caddyfile_path).is_err();
        return record_exposure_failure(
            connection,
            input.desired.application.id.as_str(),
            Visibility::Internal,
            ExposureMaterializationState::Removing,
            "exposure_changed",
            "exposure changed while Caddy route removal was being confirmed",
            recovery_failed,
        );
    }
    transaction.commit().map_err(persistence_error)?;
    Ok(ReconciliationResult::ExposureRepaired)
}

// Materializes the canonical public route, verifies external health, then persists confirmation.
fn materialize_public_route(
    connection: &mut Connection,
    input: &ReconciliationInput,
    expected_state: ExposureMaterializationState,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
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
    reserve_exposure(
        connection,
        input.desired.application.id.as_str(),
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
        Err(source) => {
            return record_exposure_failure(
                connection,
                input.desired.application.id.as_str(),
                Visibility::Public,
                ExposureMaterializationState::Applying,
                "caddy_materialization_failed",
                &source.to_string(),
                source.recovery_failed(),
            );
        }
    };
    let specification = input.persisted.specification.as_ref().ok_or_else(|| {
        ReconciliationReadError::NotConverged {
            reason: "application has no deployment specification for public health".to_owned(),
        }
    })?;
    if let Err(source) = check_external_health(
        domain,
        specification.runtime.health_check().path(),
        specification.runtime.health_check().expected_status(),
    ) {
        let recovery_failed =
            restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
        return record_exposure_failure(
            connection,
            input.desired.application.id.as_str(),
            Visibility::Public,
            ExposureMaterializationState::Applying,
            "external_health_check_failed",
            &source.to_string(),
            recovery_failed,
        );
    }
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
        let recovery_failed =
            restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
        return record_exposure_failure(
            connection,
            input.desired.application.id.as_str(),
            Visibility::Public,
            ExposureMaterializationState::Applying,
            "exposure_changed",
            "exposure changed while Caddy route materialization was being confirmed",
            recovery_failed,
        );
    }
    transaction.commit().map_err(persistence_error)?;
    Ok(ReconciliationResult::ExposureRepaired)
}

fn record_public_exposure_failure(
    connection: &Connection,
    input: &ReconciliationInput,
    failure: &PublicExposureFailure,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    record_exposure_failure(
        connection,
        input.desired.application.id.as_str(),
        Visibility::Public,
        failure.expected_state,
        failure.kind.code(),
        failure.kind.message(),
        false,
    )
}

fn reserve_exposure(
    connection: &Connection,
    application_id: &str,
    visibility: Visibility,
    state: ExposureMaterializationState,
) -> Result<(), ReconciliationReadError> {
    let outcome = match visibility {
        Visibility::Public => exposure_store::begin_public_exposure_reconciliation(
            connection,
            &ApplicationId::from(application_id),
            state,
        ),
        Visibility::Internal => exposure_store::begin_internal_exposure_reconciliation(
            connection,
            &ApplicationId::from(application_id),
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

fn record_exposure_failure(
    connection: &Connection,
    application_id: &str,
    visibility: Visibility,
    expected_state: ExposureMaterializationState,
    code: &str,
    message: &str,
    diverged: bool,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let diagnostic = ExposureDiagnostic::new(code, message).map_err(|_| {
        ReconciliationReadError::NotConverged {
            reason: "reconciliation produced an invalid exposure diagnostic".to_owned(),
        }
    })?;
    let state = if diverged {
        ExposureMaterializationState::Diverged
    } else {
        ExposureMaterializationState::Failed
    };
    let outcome = exposure_store::record_reconciliation_exposure_failure(
        connection,
        &ApplicationId::from(application_id),
        visibility,
        expected_state,
        state,
        &diagnostic,
    )
    .map_err(|source| ReconciliationReadError::Exposure { source })?;
    if outcome == PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: "exposure changed before reconciliation failure could be recorded".to_owned(),
        });
    }
    if diverged {
        Ok(ReconciliationResult::Diverged {
            reason: message.to_owned(),
        })
    } else {
        Ok(ReconciliationResult::Failed {
            reason: message.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unhandled_drift_reason_names_the_missing_automatic_repair() {
        let reason = reconciliation_decision_reason(ReconciliationDecisionError::UnhandledDrift);
        assert_eq!(
            reason,
            "drift has no automatic repair; manual intervention is required"
        );
    }

    #[test]
    fn invalid_route_fragment_reason_preserves_the_source_message() {
        let error = ReconciliationDecisionError::InvalidRouteFragment(
            crate::domain::exposure::InvalidExposureConfigurationVersion {
                value: "<invalid>".to_owned(),
            },
        );
        let reason = reconciliation_decision_reason(error);
        assert_eq!(reason, "invalid exposure configuration version `<invalid>`");
    }
}
