use rusqlite::Connection;

use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::adapters::local_runtime::observe_named_container;
use crate::adapters::stores::{PersistenceOutcome, runtime_store};
use crate::adapters::systemd_quadlet::{
    container_name, daemon_reload, start, unit_name, write_unit,
};
use crate::domain::application::{ApplicationDeploymentSpecification, ApplicationName};
use crate::domain::reconciliation::{
    ActiveRuntime, NamedContainerObservation, ReconciliationExpectations, ReconciliationInput,
    RuntimeIdentityRepair, RuntimeRematerialization,
};
use crate::domain::runtime::{ContainerId, RuntimeInstance};

use super::{ReconciliationReadError, ReconciliationResult, host_port, required_active_runtime};

// Confirms a proven recreated container by swapping the recorded container id under CAS.
pub(crate) fn confirm_runtime_identity(
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

// Rematerializes the decided runtime from persisted identity and confirms it before persisting.
//
// Stages: bring the unit up from persisted identity, prove that the started container
// carries exactly that identity, verify its loopback health, then swap the recorded
// container id under CAS. Each refusal names its own terminal result.
pub(crate) fn rematerialize_runtime(
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
