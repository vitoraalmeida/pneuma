use rusqlite::Connection;

use crate::adapters::caddy_exposure::{
    MaterializeCaddyFragmentError, MaterializedCaddyFragment, canonical_fragment_contents,
    materialize_caddy_fragment, remove_caddy_fragment, restore_materialized_caddy_fragment,
    restore_removed_caddy_fragment,
};
use crate::adapters::health_check_external::check_external_health;
use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::adapters::local_runtime::observe_named_container;
use crate::adapters::stores::{PersistenceOutcome, exposure_store, runtime_store};
use crate::adapters::systemd_quadlet::{
    container_name, daemon_reload, start, unit_name, write_unit,
};
use crate::domain::application::{ApplicationDeploymentSpecification, ApplicationName};
use crate::domain::exposure::{
    DomainName, ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent,
    ExposureMaterializationState, Visibility,
};
use crate::domain::identity::ApplicationId;
use crate::domain::reconciliation::{
    ActiveRuntime, NamedContainerObservation, PublicExposureFailure, ReconciliationDecision,
    ReconciliationDecisionError, ReconciliationExpectations, ReconciliationInput,
    RuntimeIdentityRepair, RuntimeRematerialization,
};
use crate::domain::runtime::{ContainerId, RuntimeInstance};

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
    let (_, runtime) = required_active_runtime(input)?;
    swap_recorded_container_id(
        connection,
        runtime,
        &repair.container_id,
        "identity reconciliation",
    )
}

// Swaps the recorded container id under compare-and-set; zero updated rows mean the
// logical runtime changed concurrently and reconciliation must refuse to converge.
fn swap_recorded_container_id(
    connection: &Connection,
    runtime: &RuntimeInstance,
    replacement_container_id: &ContainerId,
    interrupted_operation: &str,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let outcome = runtime_store::reconcile_external_runtime_id(
        connection,
        &runtime.id,
        &runtime.external_runtime_id,
        replacement_container_id,
    )
    .map_err(|source| ReconciliationReadError::Runtime { source })?;
    if outcome == PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: format!(
                "runtime `{}` changed before {interrupted_operation}",
                runtime.id
            ),
        });
    }
    Ok(ReconciliationResult::Repaired {
        runtime_id: runtime.id.to_string(),
        container_id: replacement_container_id.to_string(),
    })
}

// Returns the deployment specification every rematerialization verifies health against.
fn required_deployment_specification(
    input: &ReconciliationInput,
) -> Result<&ApplicationDeploymentSpecification, ReconciliationReadError> {
    input
        .persisted
        .specification
        .as_ref()
        .ok_or_else(|| ReconciliationReadError::NotConverged {
            reason: "application has no persisted deployment specification".to_owned(),
        })
}

// Rematerializes the decided runtime from persisted identity and confirms it before persisting.
//
// Stages: bring the unit up from persisted identity, prove that the started container
// carries exactly that identity, verify its loopback health, then swap the recorded
// container id under CAS. Each refusal names its own terminal result.
fn rematerialize_runtime(
    connection: &Connection,
    input: &ReconciliationInput,
    expectations: &ReconciliationExpectations,
    plan: RuntimeRematerialization,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let application = &input.desired.application;
    let (active, runtime) = required_active_runtime(input)?;
    let specification = required_deployment_specification(input)?;

    materialize_unit(&application.name, active, runtime, plan.unit_needs_write)?;

    let container_id =
        match observe_started_identity(&application.name, active, runtime, expectations)? {
            ObservedRematerialization::MatchesPersistedIdentity(container_id) => container_id,
            ObservedRematerialization::ContainerStillMissing => {
                return Ok(ReconciliationResult::Failed {
                    reason: "rematerialized Quadlet did not create its expected container"
                        .to_owned(),
                });
            }
            ObservedRematerialization::IdentityDiffersFromPersistedIntent => {
                return Ok(ReconciliationResult::ManualIntervention {
                reason:
                    "rematerialized container identity or endpoint differs from persisted intent"
                        .to_owned(),
            });
            }
        };

    if let HealthCheckResult::Unhealthy { failure, .. } =
        verify_rematerialized_health(runtime, specification)?
    {
        return Ok(ReconciliationResult::Failed {
            reason: format!("rematerialized runtime failed its internal health check: {failure:?}"),
        });
    }

    swap_recorded_container_id(
        connection,
        runtime,
        &container_id,
        "rematerialization could be confirmed",
    )
}

// Rewrites the Quadlet when the plan demands it and brings the unit up through systemd.
fn materialize_unit(
    application_name: &ApplicationName,
    active: &ActiveRuntime,
    runtime: &RuntimeInstance,
    unit_needs_write: bool,
) -> Result<(), ReconciliationReadError> {
    if unit_needs_write {
        write_unit(
            application_name,
            &active.deployment.id,
            &active.release.artifact,
            runtime.container_port,
            host_port(runtime)?,
        )
        .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
        daemon_reload().map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    }
    let unit = unit_name(application_name, &active.deployment.id);
    start(&unit).map_err(|source| ReconciliationReadError::ObserveQuadlet { source })
}

// Verdict of comparing the started container against the persisted runtime identity.
enum ObservedRematerialization {
    MatchesPersistedIdentity(ContainerId),
    ContainerStillMissing,
    IdentityDiffersFromPersistedIntent,
}

// Observes the started container and classifies it against the persisted identity
// through the one centralized matching predicate shared with planning.
fn observe_started_identity(
    application_name: &ApplicationName,
    active: &ActiveRuntime,
    runtime: &RuntimeInstance,
    expectations: &ReconciliationExpectations,
) -> Result<ObservedRematerialization, ReconciliationReadError> {
    let observed = observe_named_container(
        &container_name(application_name, &active.deployment.id),
        runtime.container_port,
    )
    .map_err(|source| ReconciliationReadError::ObserveNamedContainer { source })?;
    let NamedContainerObservation::Present { id, .. } = &observed else {
        return Ok(ObservedRematerialization::ContainerStillMissing);
    };
    if observed.matches_expected_runtime(
        &expectations.container_name,
        &active.release.artifact,
        application_name.as_str(),
        runtime.expected_endpoint.socket_addr(),
    ) {
        Ok(ObservedRematerialization::MatchesPersistedIdentity(
            id.clone(),
        ))
    } else {
        Ok(ObservedRematerialization::IdentityDiffersFromPersistedIntent)
    }
}

// Checks the started runtime against its persisted loopback health specification.
fn verify_rematerialized_health(
    runtime: &RuntimeInstance,
    specification: &ApplicationDeploymentSpecification,
) -> Result<HealthCheckResult, ReconciliationReadError> {
    check_internal_health(
        runtime.expected_endpoint.socket_addr(),
        specification.runtime.health_check(),
    )
    .map_err(|source| ReconciliationReadError::NotConverged {
        reason: source.to_string(),
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
//
// Stages: prepare and reserve the exposure snapshot, apply the canonical Caddy
// fragment, verify the route through its public domain, then confirm the change
// under CAS. Every compensated phase restores the prior route before recording
// its diagnostic.
fn materialize_public_route(
    connection: &mut Connection,
    input: &ReconciliationInput,
    expected_state: ExposureMaterializationState,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let (domain, runtime, configuration_version) = prepare_public_route(input)?;
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
        input.desired.application.id.as_str(),
        Visibility::Public,
        ExposureMaterializationState::Applying,
        "caddy_materialization_failed",
        &source.to_string(),
        source.recovery_failed(),
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
        let recovery_failed =
            restore_materialized_caddy_fragment(materialized, caddyfile_path).is_err();
        return record_exposure_failure(
            connection,
            input.desired.application.id.as_str(),
            Visibility::Public,
            ExposureMaterializationState::Applying,
            "external_health_check_failed",
            &source.to_string(),
            recovery_failed,
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
        let recovery_failed =
            restore_materialized_caddy_fragment(materialized, caddyfile_path).is_err();
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
