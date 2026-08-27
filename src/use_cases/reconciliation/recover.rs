use rusqlite::Connection;

use crate::adapters::caddy_exposure::observe_caddy_fragment;
use crate::adapters::local_runtime::observe_named_container;
use crate::adapters::stores::{PersistenceOutcome, exposure_store, release_store, runtime_store};
use crate::adapters::systemd_quadlet::{
    canonical_unit_contents, container_name, observe_unit_source, unit_name,
};
use crate::domain::application::Application;
use crate::domain::deployment::{Deployment, DeploymentFailureCode, DeploymentStatus};
use crate::domain::exposure::{Exposure, ExposureDiagnostic, ExposureMaterializationState};
use crate::domain::identity::ApplicationId;
use crate::domain::reconciliation::{
    ActiveRuntime, CaddyFragmentObservation, NamedContainerObservation, QuadletSourceObservation,
};
use crate::use_cases::deployment::{cleanup_failed_candidate, fail_deployment};

use super::{ReconciliationReadError, ReconciliationResult};

// Terminates work left by a dead lock holder without treating an incomplete candidate as promotable.
pub(crate) fn recover_interrupted_deployment(
    connection: &mut Connection,
    application: &Application,
    active: Option<&ActiveRuntime>,
    exposure: Option<&Exposure>,
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
                runtime_store::load_runtime_by_deployment(connection, &deployment.id)
                    .map_err(|source| ReconciliationReadError::Runtime { source })?
            else {
                return Ok(ReconciliationResult::ManualIntervention {
                    reason: "interrupted deployment has no persisted candidate runtime to prove cleanup ownership".to_owned(),
                });
            };
            let release = release_store::load_release_by_id(connection, &deployment.release_id)
                .map_err(|source| ReconciliationReadError::Release { source })?;
            let unit = unit_name(&application.name, &deployment.id);
            let expected_unit = canonical_unit_contents(
                &application.name,
                &deployment.id,
                &release.artifact,
                runtime.container_port,
                runtime
                    .expected_endpoint
                    .host_port()
                    .map_err(|source| ReconciliationReadError::InvalidExpectedPort { source })?,
            );
            let unit_proven = observe_unit_source(&unit)
                .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?
                == QuadletSourceObservation::Present {
                    contents: expected_unit,
                };
            let container_proven = match observe_named_container(
                &container_name(&application.name, &deployment.id),
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
                            == container_name(&application.name, &deployment.id)
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
                &deployment.id,
                unit_proven.then_some(unit.as_str()),
                container_proven.then_some(&runtime.external_runtime_id),
                Some(&runtime.id),
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
                &application.id,
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
            let outcome = exposure_store::record_reconciliation_exposure_failure(
                connection,
                &application.id,
                exposure.intent().visibility(),
                ExposureMaterializationState::Applying,
                state,
                &diagnostic,
            )
            .map_err(|source| ReconciliationReadError::Exposure { source })?;
            if outcome == PersistenceOutcome::Stale {
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
        &deployment.id,
        DeploymentFailureCode::OperationInterrupted.as_str(),
        "operation owner exited before deployment completion",
    )
    .map_err(|source| ReconciliationReadError::NotConverged {
        reason: format!("interrupted deployment failure could not be recorded: {source}"),
    })?;
    Ok(())
}

fn prior_canonical_route_is_present(
    active: Option<&ActiveRuntime>,
    exposure: Option<&Exposure>,
    managed_caddy_directory: &std::path::Path,
    application_id: &ApplicationId,
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
