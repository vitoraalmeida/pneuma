use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::application_lock::{ApplicationLock, ApplicationLockError};
use crate::adapters::caddy_exposure::{
    ObserveCaddyFragmentError, materialize_caddy_fragment, observe_caddy_fragment,
    remove_caddy_fragment, restore_materialized_caddy_fragment, restore_removed_caddy_fragment,
};
use crate::adapters::health_check_external::check_external_health;
use crate::adapters::health_check_internal::{HealthCheckResult, check_internal_health};
use crate::adapters::local_runtime::{
    ObserveContainerError, ObserveNamedContainerError, observe_container, observe_named_container,
};
use crate::adapters::stores::operation_store::{self, OperationStoreError};
use crate::adapters::stores::{application_store, deployment_store, release_store, runtime_store};
use crate::adapters::systemd_quadlet::{
    QuadletError, canonical_unit_contents, container_name, daemon_reload, observe_generated_unit,
    observe_unit_source, start, unit_name, write_unit,
};
use crate::domain::deployment::{Deployment, DeploymentStatus};
use crate::domain::exposure::{
    ExposureConfigurationVersion, ExposureDiagnostic, ExposureMaterializationState, Visibility,
};
use crate::domain::reconciliation::{
    ActiveRuntime, CaddyFragmentObservation, NamedContainerObservation, ReconciliationInput,
    ReconciliationObservation,
};
use crate::domain::runtime::{DesiredRuntimeState, ObservedRuntimeState};
use crate::use_cases::deployment_runtime_cleanup::cleanup_failed_candidate;
use crate::use_cases::deployment_transition::fail_deployment;

#[derive(Debug)]
pub enum ReconciliationResult {
    NoOp,
    Deferred {
        blocking_deployment: Option<Box<Deployment>>,
    },
    Repaired {
        runtime_id: String,
        container_id: String,
    },
    ManualIntervention {
        reason: String,
    },
    ExposureRepaired,
    Failed {
        reason: String,
    },
    Diverged {
        reason: String,
    },
}

#[derive(Debug)]
pub enum ReconciliationReadError {
    ApplicationNotFound {
        application_name: String,
    },
    Application {
        source: application_store::ApplicationStoreError,
    },
    Deployment {
        source: deployment_store::DeploymentStoreError,
    },
    Release {
        source: release_store::ReleaseStoreError,
    },
    Runtime {
        source: runtime_store::RuntimeStoreError,
    },
    Exposure {
        source: application_store::ExposureStoreError,
    },
    OperationLock {
        source: ApplicationLockError,
    },
    Operation {
        source: OperationStoreError,
    },
    ObserveContainer {
        source: ObserveContainerError,
    },
    ObserveNamedContainer {
        source: ObserveNamedContainerError,
    },
    ObserveQuadlet {
        source: QuadletError,
    },
    ObserveCaddy {
        source: ObserveCaddyFragmentError,
    },
    NotConverged {
        reason: String,
    },
}

impl fmt::Display for ReconciliationReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_name } => {
                write!(formatter, "application `{application_name}` was not found")
            }
            Self::Application { source } => write!(
                formatter,
                "failed to load reconciliation application: {source}"
            ),
            Self::Deployment { source } => write!(
                formatter,
                "failed to load reconciliation deployment: {source}"
            ),
            Self::Release { source } => {
                write!(formatter, "failed to load reconciliation release: {source}")
            }
            Self::Runtime { source } => {
                write!(formatter, "failed to load reconciliation runtime: {source}")
            }
            Self::Exposure { source } => write!(
                formatter,
                "failed to load reconciliation exposure: {source}"
            ),
            Self::OperationLock { source } => {
                write!(formatter, "failed to serialize reconciliation: {source}")
            }
            Self::Operation { source } => {
                write!(
                    formatter,
                    "failed to acquire reconciliation ownership: {source}"
                )
            }
            Self::ObserveContainer { source } => {
                write!(formatter, "failed to observe recorded runtime: {source}")
            }
            Self::ObserveNamedContainer { source } => {
                write!(formatter, "failed to observe named runtime: {source}")
            }
            Self::ObserveQuadlet { source } => {
                write!(formatter, "failed to observe Quadlet: {source}")
            }
            Self::ObserveCaddy { source } => {
                write!(formatter, "failed to observe Caddy fragment: {source}")
            }
            Self::NotConverged { reason } => {
                write!(formatter, "reconciliation is not yet converged: {reason}")
            }
        }
    }
}

impl Error for ReconciliationReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Application { source } => Some(source),
            Self::Deployment { source } => Some(source),
            Self::Release { source } => Some(source),
            Self::Runtime { source } => Some(source),
            Self::Exposure { source } => Some(source),
            Self::OperationLock { source } => Some(source),
            Self::Operation { source } => Some(source),
            Self::ObserveContainer { source } => Some(source),
            Self::ObserveNamedContainer { source } => Some(source),
            Self::ObserveQuadlet { source } => Some(source),
            Self::ObserveCaddy { source } => Some(source),
            Self::ApplicationNotFound { .. } | Self::NotConverged { .. } => None,
        }
    }
}

// Reconciles only confirmed runtime and route drift, leaving ambiguous materialization untouched.
pub fn reconcile_application(
    connection: &mut Connection,
    application_name: &str,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    let application = application_store::load_application_by_name(connection, application_name)
        .map_err(|source| ReconciliationReadError::Application { source })?
        .ok_or_else(|| ReconciliationReadError::ApplicationNotFound {
            application_name: application_name.to_owned(),
        })?;
    let database_path = connection.path().map(std::path::PathBuf::from).ok_or(
        ReconciliationReadError::OperationLock {
            source: ApplicationLockError::DatabasePathUnavailable,
        },
    )?;
    let Some(_lock) = ApplicationLock::try_acquire(&database_path, application.id.as_str())
        .map_err(|source| ReconciliationReadError::OperationLock { source })?
    else {
        return Ok(ReconciliationResult::Deferred {
            blocking_deployment: None,
        });
    };
    let token = operation_store::generate_token(connection)
        .map_err(|source| ReconciliationReadError::Operation { source })?;
    let transaction = connection.transaction().map_err(persistence_error)?;
    operation_store::take_ownership(&transaction, application.id.as_str(), &token)
        .map_err(|source| ReconciliationReadError::Operation { source })?;
    transaction.commit().map_err(persistence_error)?;

    let input = load_reconciliation_input(connection, application_name)?;
    if let Some(blocking_deployment) = input.blocking_deployment {
        return recover_interrupted_deployment(
            connection,
            &input.application,
            input.active.as_ref(),
            input.exposure.as_ref(),
            &blocking_deployment,
            managed_caddy_directory,
        );
    }
    let observation = observe_reconciliation_input(&input, managed_caddy_directory)?;
    let Some(observation) = observation else {
        return Err(ReconciliationReadError::NotConverged {
            reason: "application has no active runtime".to_owned(),
        });
    };
    if input.application.desired_runtime_state == DesiredRuntimeState::Stopped
        && *observation.recorded_container.state() == ObservedRuntimeState::Missing
        && observation.named_container == NamedContainerObservation::Missing
        && observation.caddy_fragment == CaddyFragmentObservation::Missing
    {
        return Ok(ReconciliationResult::NoOp);
    }
    if let Some(repaired) = repair_recreated_runtime(connection, &input, &observation)? {
        return Ok(repaired);
    }
    if let Some(repaired) = rematerialize_missing_runtime(connection, &input, &observation)? {
        return Ok(repaired);
    }
    if let Some(result) = reconcile_exposure(
        connection,
        &input,
        &observation,
        managed_caddy_directory,
        caddyfile_path,
    )? {
        return Ok(result);
    }
    if input.application.desired_runtime_state == DesiredRuntimeState::Running {
        return Ok(ReconciliationResult::ManualIntervention {
            reason: "runtime identity or configuration differs from persisted intent".to_owned(),
        });
    }
    Err(ReconciliationReadError::NotConverged {
        reason: "runtime repair and public-route confirmation are not implemented".to_owned(),
    })
}

// Recreates an absent canonical Quadlet only from the active runtime's persisted identity.
fn rematerialize_missing_runtime(
    connection: &Connection,
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
) -> Result<Option<ReconciliationResult>, ReconciliationReadError> {
    let generated_unit_is_absent = match &observation.systemd_unit {
        crate::domain::reconciliation::SystemdUnitObservation::Missing => true,
        crate::domain::reconciliation::SystemdUnitObservation::Present { active_state } => {
            active_state == "inactive"
        }
    };
    if input.application.desired_runtime_state != DesiredRuntimeState::Running
        || *observation.recorded_container.state() != ObservedRuntimeState::Missing
        || observation.named_container != NamedContainerObservation::Missing
        || observation.quadlet_source
            != crate::domain::reconciliation::QuadletSourceObservation::Missing
        || !generated_unit_is_absent
    {
        return Ok(None);
    }
    let (Some(active), Some(specification)) = (&input.active, &input.specification) else {
        return Ok(None);
    };
    let Some(runtime) = &active.runtime else {
        return Ok(None);
    };
    let unit = write_unit(
        input.application.name.as_str(),
        active.deployment.id.as_str(),
        active.release.artifact.reference(),
        runtime.container_port,
        runtime.expected_endpoint.socket_addr().port(),
        active.release.artifact.digest(),
    )
    .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    daemon_reload().map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    start(&unit).map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    let NamedContainerObservation::Present {
        id,
        name,
        image_reference,
        application_label,
        image_digest_label,
        observation: container_observation,
    } = observe_named_container(
        &container_name(
            input.application.name.as_str(),
            active.deployment.id.as_str(),
        ),
        runtime.container_port,
    )
    .map_err(|source| ReconciliationReadError::ObserveNamedContainer { source })?
    else {
        return Ok(Some(ReconciliationResult::Failed {
            reason: "rematerialized Quadlet did not create its expected container".to_owned(),
        }));
    };
    if *container_observation.state() != ObservedRuntimeState::Running
        || name.trim_start_matches('/')
            != container_name(
                input.application.name.as_str(),
                active.deployment.id.as_str(),
            )
        || image_reference != active.release.artifact.reference()
        || application_label.as_deref() != Some(input.application.name.as_str())
        || image_digest_label.as_deref() != Some(active.release.artifact.digest())
        || container_observation.observed_endpoint()
            != Some(runtime.expected_endpoint.socket_addr())
    {
        return Ok(Some(ReconciliationResult::ManualIntervention {
            reason: "rematerialized container identity or endpoint differs from persisted intent"
                .to_owned(),
        }));
    }
    match check_internal_health(
        runtime.expected_endpoint.socket_addr(),
        specification.runtime.health_check().path().as_str(),
        specification.runtime.health_check().expected_status().get(),
    )
    .map_err(|source| ReconciliationReadError::NotConverged {
        reason: source.to_string(),
    })? {
        HealthCheckResult::Healthy { .. } => {}
        HealthCheckResult::Unhealthy { failure, .. } => {
            return Ok(Some(ReconciliationResult::Failed {
                reason: format!(
                    "rematerialized runtime failed its internal health check: {failure:?}"
                ),
            }));
        }
    }
    let outcome = runtime_store::reconcile_external_runtime_id(
        connection,
        runtime.id.as_str(),
        runtime.external_runtime_id.as_str(),
        id.as_str(),
    )
    .map_err(|source| ReconciliationReadError::Runtime { source })?;
    if outcome == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: format!(
                "runtime `{}` changed before rematerialization could be confirmed",
                runtime.id
            ),
        });
    }
    Ok(Some(ReconciliationResult::Repaired {
        runtime_id: runtime.id.to_string(),
        container_id: id.to_string(),
    }))
}

// Terminates work left by a dead lock holder without treating an incomplete candidate as promotable.
fn recover_interrupted_deployment(
    connection: &mut Connection,
    application: &crate::domain::application::Application,
    active: Option<&ActiveRuntime>,
    exposure: Option<&crate::domain::exposure::Exposure>,
    deployment: &Deployment,
    managed_caddy_directory: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    match deployment.status() {
        DeploymentStatus::Pending => {
            record_interrupted_failure(connection, deployment)?;
            Ok(ReconciliationResult::Failed {
                reason: "interrupted pending deployment had no confirmed external effects"
                    .to_owned(),
            })
        }
        DeploymentStatus::Starting | DeploymentStatus::Verifying => {
            record_interrupted_failure(connection, deployment)?;
            let Some(runtime) =
                runtime_store::load_runtime_by_deployment(connection, deployment.id.as_str())
                    .map_err(|source| ReconciliationReadError::Runtime { source })?
            else {
                return Ok(ReconciliationResult::ManualIntervention {
                    reason: "interrupted deployment has no persisted candidate runtime to prove cleanup ownership".to_owned(),
                });
            };
            let release =
                release_store::load_release_by_id(connection, deployment.release_id.as_str())
                    .map_err(|source| ReconciliationReadError::Release { source })?;
            let unit = unit_name(application.name.as_str(), deployment.id.as_str());
            let expected_unit = canonical_unit_contents(
                application.name.as_str(),
                deployment.id.as_str(),
                release.artifact.reference(),
                runtime.container_port,
                runtime.expected_endpoint.socket_addr().port(),
                release.artifact.digest(),
            );
            let unit_proven = observe_unit_source(&unit)
                .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?
                == crate::domain::reconciliation::QuadletSourceObservation::Present {
                    contents: expected_unit,
                };
            let container_proven = match observe_named_container(
                &container_name(application.name.as_str(), deployment.id.as_str()),
                runtime.container_port,
            )
            .map_err(|source| ReconciliationReadError::ObserveNamedContainer { source })?
            {
                NamedContainerObservation::Missing => false,
                NamedContainerObservation::Present {
                    id,
                    name,
                    image_reference,
                    application_label,
                    image_digest_label,
                    observation,
                } => {
                    id == runtime.external_runtime_id
                        && name.trim_start_matches('/')
                            == container_name(application.name.as_str(), deployment.id.as_str())
                        && image_reference == release.artifact.reference()
                        && application_label.as_deref() == Some(application.name.as_str())
                        && image_digest_label.as_deref() == Some(release.artifact.digest())
                        && observation.observed_endpoint()
                            == Some(runtime.expected_endpoint.socket_addr())
                }
            };
            if !unit_proven && !container_proven {
                return Ok(ReconciliationResult::ManualIntervention {
                    reason:
                        "interrupted candidate identity cannot be proven; no cleanup was attempted"
                            .to_owned(),
                });
            }
            cleanup_failed_candidate(
                connection,
                deployment.id.as_str(),
                unit_proven.then_some(unit.as_str()),
                container_proven.then_some(runtime.external_runtime_id.as_str()),
                Some(runtime.id.as_str()),
            )
            .map_err(|source| ReconciliationReadError::NotConverged {
                reason: format!("interrupted candidate cleanup was incomplete: {source}"),
            })?;
            Ok(ReconciliationResult::Failed {
                reason: "interrupted candidate was cleaned up without promotion".to_owned(),
            })
        }
        DeploymentStatus::Activating => {
            record_interrupted_failure(connection, deployment)?;
            let route_is_prior_canonical = prior_canonical_route_is_present(
                active,
                exposure,
                managed_caddy_directory,
                application.id.as_str(),
            )?;
            let Some(exposure) = exposure else {
                return Ok(ReconciliationResult::ManualIntervention {
                    reason: "interrupted activation has no persisted exposure evidence".to_owned(),
                });
            };
            let diagnostic = ExposureDiagnostic::new(
                "interrupted_activation",
                if route_is_prior_canonical {
                    "activation was interrupted; the prior canonical route is preserved"
                } else {
                    "activation was interrupted and the prior canonical route cannot be proven"
                },
            )
            .map_err(|_| ReconciliationReadError::NotConverged {
                reason: "interrupted activation produced an invalid exposure diagnostic".to_owned(),
            })?;
            let state = if route_is_prior_canonical {
                ExposureMaterializationState::Failed
            } else {
                ExposureMaterializationState::Diverged
            };
            let outcome = application_store::record_reconciliation_exposure_failure(
                connection,
                application.id.as_str(),
                exposure.intent().visibility(),
                ExposureMaterializationState::Applying,
                state,
                &diagnostic,
            )
            .map_err(|source| ReconciliationReadError::Application { source })?;
            if outcome == crate::adapters::stores::PersistenceOutcome::Stale {
                return Ok(ReconciliationResult::ManualIntervention {
                    reason:
                        "interrupted activation exposure changed before recovery could record it"
                            .to_owned(),
                });
            }
            if route_is_prior_canonical {
                Ok(ReconciliationResult::Failed {
                    reason: "interrupted activation was not promoted; the prior route is preserved"
                        .to_owned(),
                })
            } else {
                Ok(ReconciliationResult::Diverged {
                    reason: "interrupted activation was not promoted and route state is unproven"
                        .to_owned(),
                })
            }
        }
        DeploymentStatus::Succeeded | DeploymentStatus::Failed => unreachable!(),
    }
}

fn record_interrupted_failure(
    connection: &mut Connection,
    deployment: &Deployment,
) -> Result<(), ReconciliationReadError> {
    fail_deployment(
        connection,
        deployment.id.as_str(),
        "operation_interrupted",
        "operation owner exited before deployment completion",
    )
    .map_err(|source| ReconciliationReadError::NotConverged {
        reason: format!("interrupted deployment failure could not be recorded: {source}"),
    })?;
    Ok(())
}

fn prior_canonical_route_is_present(
    active: Option<&ActiveRuntime>,
    exposure: Option<&crate::domain::exposure::Exposure>,
    managed_caddy_directory: &std::path::Path,
    application_id: &str,
) -> Result<bool, ReconciliationReadError> {
    let (Some(active), Some(exposure)) = (active, exposure) else {
        return Ok(false);
    };
    let (Some(runtime), Some(route)) = (
        &active.runtime,
        exposure.materialization().confirmed_route(),
    ) else {
        return Ok(false);
    };
    if route.runtime_id() != &runtime.id {
        return Ok(false);
    }
    Ok(
        observe_caddy_fragment(managed_caddy_directory, application_id)
            .map_err(|source| ReconciliationReadError::ObserveCaddy { source })?
            == CaddyFragmentObservation::Present {
                contents: route.configuration_version().as_str().to_owned(),
            },
    )
}

// Reconciles a managed route only after the persisted snapshot has been CAS-reserved.
fn reconcile_exposure(
    connection: &mut Connection,
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<Option<ReconciliationResult>, ReconciliationReadError> {
    let Some(exposure) = &input.exposure else {
        return Ok(None);
    };
    let state = exposure.materialization().state();
    if state == ExposureMaterializationState::Diverged {
        return Ok(Some(ReconciliationResult::ManualIntervention {
            reason: "exposure materialization diverged and requires manual intervention".to_owned(),
        }));
    }
    match exposure.intent().visibility() {
        Visibility::Internal => {
            if observation.caddy_fragment == CaddyFragmentObservation::Missing {
                return Ok(None);
            }
            reserve_exposure(
                connection,
                input.application.id.as_str(),
                Visibility::Internal,
                state,
            )?;
            let removed = match remove_caddy_fragment(
                managed_caddy_directory,
                input.application.id.as_str(),
                caddyfile_path,
            ) {
                Ok(removed) => removed,
                Err(source) => {
                    return record_exposure_failure(
                        connection,
                        input.application.id.as_str(),
                        Visibility::Internal,
                        ExposureMaterializationState::Removing,
                        "caddy_removal_failed",
                        &source.to_string(),
                        source.recovery_failed(),
                    );
                }
            };
            let transaction = connection.transaction().map_err(persistence_error)?;
            let completed = application_store::complete_internal_exposure_change(
                &transaction,
                input.application.id.as_str(),
            )
            .map_err(|source| ReconciliationReadError::Application { source })?;
            if completed == crate::adapters::stores::PersistenceOutcome::Stale {
                drop(transaction);
                let recovery_failed =
                    restore_removed_caddy_fragment(&removed, caddyfile_path).is_err();
                return record_exposure_failure(
                    connection,
                    input.application.id.as_str(),
                    Visibility::Internal,
                    ExposureMaterializationState::Removing,
                    "exposure_changed",
                    "exposure changed while Caddy route removal was being confirmed",
                    recovery_failed,
                );
            }
            transaction.commit().map_err(persistence_error)?;
            Ok(Some(ReconciliationResult::ExposureRepaired))
        }
        Visibility::Public => reconcile_public_exposure(
            connection,
            input,
            observation,
            managed_caddy_directory,
            caddyfile_path,
            state,
        ),
    }
}

fn reconcile_public_exposure(
    connection: &mut Connection,
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
    state: ExposureMaterializationState,
) -> Result<Option<ReconciliationResult>, ReconciliationReadError> {
    let Some(exposure) = &input.exposure else {
        return Ok(None);
    };
    let Some(domain) = exposure.intent().domain() else {
        return record_exposure_failure(
            connection,
            input.application.id.as_str(),
            Visibility::Public,
            state,
            "domain_missing",
            "public exposure has no configured domain",
            false,
        );
    };
    let Some(active) = &input.active else {
        return record_exposure_failure(
            connection,
            input.application.id.as_str(),
            Visibility::Public,
            state,
            "runtime_missing",
            "public exposure has no active runtime",
            false,
        );
    };
    let Some(runtime) = &active.runtime else {
        return record_exposure_failure(
            connection,
            input.application.id.as_str(),
            Visibility::Public,
            state,
            "runtime_missing",
            "public exposure has no active runtime",
            false,
        );
    };
    let endpoint = runtime.expected_endpoint.socket_addr();
    if input.application.desired_runtime_state != DesiredRuntimeState::Running
        || *observation.recorded_container.state() != ObservedRuntimeState::Running
        || observation.recorded_container.observed_endpoint() != Some(endpoint)
    {
        return record_exposure_failure(
            connection,
            input.application.id.as_str(),
            Visibility::Public,
            state,
            "runtime_not_healthy",
            "public exposure requires a confirmed healthy running runtime",
            false,
        );
    }
    let contents =
        crate::adapters::caddy_exposure::canonical_fragment_contents(domain.as_str(), endpoint);
    let configuration_version = ExposureConfigurationVersion::new(&contents).map_err(|source| {
        ReconciliationReadError::NotConverged {
            reason: source.to_string(),
        }
    })?;
    let route_is_confirmed = exposure
        .materialization()
        .confirmed_route()
        .is_some_and(|route| {
            route.runtime_id() == &runtime.id
                && route.configuration_version() == &configuration_version
        });
    if observation.caddy_fragment == (CaddyFragmentObservation::Present { contents })
        && state == ExposureMaterializationState::Active
        && route_is_confirmed
    {
        return Ok(Some(ReconciliationResult::NoOp));
    }
    reserve_exposure(
        connection,
        input.application.id.as_str(),
        Visibility::Public,
        state,
    )?;
    let materialized = match materialize_caddy_fragment(
        managed_caddy_directory,
        caddyfile_path,
        input.application.id.as_str(),
        domain.as_str(),
        endpoint,
    ) {
        Ok(materialized) => materialized,
        Err(source) => {
            return record_exposure_failure(
                connection,
                input.application.id.as_str(),
                Visibility::Public,
                ExposureMaterializationState::Applying,
                "caddy_materialization_failed",
                &source.to_string(),
                source.recovery_failed(),
            );
        }
    };
    let specification =
        input
            .specification
            .as_ref()
            .ok_or_else(|| ReconciliationReadError::NotConverged {
                reason: "application has no deployment specification for public health".to_owned(),
            })?;
    if let Err(source) = check_external_health(
        domain.as_str(),
        specification.runtime.health_check().path().as_str(),
        specification.runtime.health_check().expected_status().get(),
    ) {
        let recovery_failed =
            restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
        return record_exposure_failure(
            connection,
            input.application.id.as_str(),
            Visibility::Public,
            ExposureMaterializationState::Applying,
            "external_health_check_failed",
            &source.to_string(),
            recovery_failed,
        );
    }
    let transaction = connection.transaction().map_err(persistence_error)?;
    let completed = application_store::complete_public_exposure_change(
        &transaction,
        input.application.id.as_str(),
        &runtime.id,
        &configuration_version,
    )
    .map_err(|source| ReconciliationReadError::Application { source })?;
    if completed == crate::adapters::stores::PersistenceOutcome::Stale {
        drop(transaction);
        let recovery_failed =
            restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
        return record_exposure_failure(
            connection,
            input.application.id.as_str(),
            Visibility::Public,
            ExposureMaterializationState::Applying,
            "exposure_changed",
            "exposure changed while Caddy route materialization was being confirmed",
            recovery_failed,
        );
    }
    transaction.commit().map_err(persistence_error)?;
    Ok(Some(ReconciliationResult::ExposureRepaired))
}

fn reserve_exposure(
    connection: &Connection,
    application_id: &str,
    visibility: Visibility,
    state: ExposureMaterializationState,
) -> Result<(), ReconciliationReadError> {
    let outcome = match visibility {
        Visibility::Public => application_store::begin_public_exposure_reconciliation(
            connection,
            application_id,
            state,
        ),
        Visibility::Internal => application_store::begin_internal_exposure_reconciliation(
            connection,
            application_id,
            state,
        ),
    }
    .map_err(|source| ReconciliationReadError::Application { source })?;
    if outcome == crate::adapters::stores::PersistenceOutcome::Stale {
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
) -> Result<Option<ReconciliationResult>, ReconciliationReadError> {
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
    let outcome = application_store::record_reconciliation_exposure_failure(
        connection,
        application_id,
        visibility,
        expected_state,
        state,
        &diagnostic,
    )
    .map_err(|source| ReconciliationReadError::Application { source })?;
    if outcome == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: "exposure changed before reconciliation failure could be recorded".to_owned(),
        });
    }
    let result = if diverged {
        ReconciliationResult::Diverged {
            reason: message.to_owned(),
        }
    } else {
        ReconciliationResult::Failed {
            reason: message.to_owned(),
        }
    };
    Ok(Some(result))
}

fn persistence_error(source: rusqlite::Error) -> ReconciliationReadError {
    ReconciliationReadError::Application {
        source: application_store::ApplicationStoreError::Persistence { source },
    }
}

// Reconciles only a fully confirmed Quadlet recreation; all other drift remains non-destructive.
fn repair_recreated_runtime(
    connection: &Connection,
    input: &ReconciliationInput,
    observation: &ReconciliationObservation,
) -> Result<Option<ReconciliationResult>, ReconciliationReadError> {
    if input.application.desired_runtime_state != DesiredRuntimeState::Running
        || observation.caddy_fragment != CaddyFragmentObservation::Missing
    {
        return Ok(None);
    }
    let (
        Some(active),
        NamedContainerObservation::Present {
            id,
            name,
            image_reference,
            application_label,
            image_digest_label,
            observation: container_observation,
        },
    ) = (&input.active, &observation.named_container)
    else {
        return Ok(None);
    };
    let Some(runtime) = &active.runtime else {
        return Ok(None);
    };
    if *observation.recorded_container.state() != ObservedRuntimeState::Missing
        || *container_observation.state() != ObservedRuntimeState::Running
        || name.trim_start_matches('/')
            != container_name(
                input.application.name.as_str(),
                active.deployment.id.as_str(),
            )
        || image_reference != active.release.artifact.reference()
        || application_label.as_deref() != Some(input.application.name.as_str())
        || image_digest_label.as_deref() != Some(active.release.artifact.digest())
        || container_observation.observed_endpoint()
            != Some(runtime.expected_endpoint.socket_addr())
    {
        return Ok(None);
    }
    let expected_unit = canonical_unit_contents(
        input.application.name.as_str(),
        active.deployment.id.as_str(),
        active.release.artifact.reference(),
        runtime.container_port,
        runtime.expected_endpoint.socket_addr().port(),
        active.release.artifact.digest(),
    );
    if observation.quadlet_source
        != (crate::domain::reconciliation::QuadletSourceObservation::Present {
            contents: expected_unit,
        })
    {
        return Ok(None);
    }
    let outcome = runtime_store::reconcile_external_runtime_id(
        connection,
        runtime.id.as_str(),
        runtime.external_runtime_id.as_str(),
        id.as_str(),
    )
    .map_err(|source| ReconciliationReadError::Runtime { source })?;
    if outcome == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(ReconciliationReadError::NotConverged {
            reason: format!(
                "runtime `{}` changed before identity reconciliation",
                runtime.id
            ),
        });
    }
    Ok(Some(ReconciliationResult::Repaired {
        runtime_id: runtime.id.to_string(),
        container_id: id.to_string(),
    }))
}

// Loads all persisted reconciliation authorities in a short read transaction before external observation.
pub fn load_reconciliation_input(
    connection: &mut Connection,
    application_name: &str,
) -> Result<ReconciliationInput, ReconciliationReadError> {
    let transaction =
        connection
            .transaction()
            .map_err(|source| ReconciliationReadError::Application {
                source: application_store::ApplicationStoreError::Persistence { source },
            })?;
    let application = application_store::load_application_by_name(&transaction, application_name)
        .map_err(|source| ReconciliationReadError::Application { source })?
        .ok_or_else(|| ReconciliationReadError::ApplicationNotFound {
            application_name: application_name.to_owned(),
        })?;
    let blocking_deployment =
        deployment_store::load_nonterminal_deployment(&transaction, application.id.as_str())
            .map_err(|source| ReconciliationReadError::Deployment { source })?;
    let exposure = application_store::load_exposure(&transaction, application.id.as_str())
        .map_err(|source| ReconciliationReadError::Exposure { source })?;
    let specification =
        application_store::load_deployment_specification(&transaction, application.id.as_str())
            .map_err(|source| ReconciliationReadError::Application { source })?;
    let active = match &application.active_deployment_id {
        Some(deployment_id) => {
            let deployment =
                deployment_store::load_deployment(&transaction, deployment_id.as_str())
                    .map_err(|source| ReconciliationReadError::Deployment { source })?;
            let release =
                release_store::load_release_by_id(&transaction, deployment.release_id.as_str())
                    .map_err(|source| ReconciliationReadError::Release { source })?;
            let runtime = runtime_store::load_current_successful_runtime(
                &transaction,
                application.id.as_str(),
            )
            .map_err(|source| ReconciliationReadError::Runtime { source })?;
            Some(ActiveRuntime {
                deployment,
                release,
                runtime,
            })
        }
        None => None,
    };
    transaction
        .commit()
        .map_err(|source| ReconciliationReadError::Application {
            source: application_store::ApplicationStoreError::Persistence { source },
        })?;
    Ok(ReconciliationInput {
        application,
        blocking_deployment,
        active,
        exposure,
        specification,
    })
}

// Observes persisted runtime and route identities without changing SQLite or controlling external resources.
pub fn observe_reconciliation_input(
    input: &ReconciliationInput,
    managed_caddy_directory: &std::path::Path,
) -> Result<Option<ReconciliationObservation>, ReconciliationReadError> {
    let Some(active) = &input.active else {
        return Ok(None);
    };
    let Some(runtime) = &active.runtime else {
        return Ok(None);
    };
    let recorded_container =
        observe_container(runtime.external_runtime_id.as_str(), runtime.container_port)
            .map_err(|source| ReconciliationReadError::ObserveContainer { source })?;
    let name = container_name(
        input.application.name.as_str(),
        active.deployment.id.as_str(),
    );
    let named_container = observe_named_container(&name, runtime.container_port)
        .map_err(|source| ReconciliationReadError::ObserveNamedContainer { source })?;
    let unit = unit_name(
        input.application.name.as_str(),
        active.deployment.id.as_str(),
    );
    let quadlet_source = observe_unit_source(&unit)
        .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    let systemd_unit = observe_generated_unit(&unit)
        .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    let caddy_fragment =
        observe_caddy_fragment(managed_caddy_directory, input.application.id.as_str())
            .map_err(|source| ReconciliationReadError::ObserveCaddy { source })?;
    Ok(Some(ReconciliationObservation {
        recorded_container,
        named_container,
        quadlet_source,
        systemd_unit,
        caddy_fragment,
    }))
}
