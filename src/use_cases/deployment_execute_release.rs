use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use rusqlite::Connection;

use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::domain::deployment::{DeploymentStatus, DeploymentType};
use crate::domain::manifest::Visibility;
use crate::domain::release::Release;
use crate::use_cases::deployment_activate_public::{
    PublicActivationError, PublicActivationInput, activate_public_candidate,
};
use crate::use_cases::deployment_create::{
    CreateDeploymentError, create_deployment_with_source_revision,
};
use crate::use_cases::deployment_progress::{DeploymentProgress, DeploymentStep, ProgressReporter};
use crate::use_cases::deployment_promote_internal::{
    PromoteInternalCandidateError, promote_internal_candidate,
};
use crate::use_cases::deployment_runtime_cleanup::{
    CandidateCleanupError, CandidateResources, cleanup_failed_candidate, load_previous_runtime,
    retire_previous_runtime,
};
use crate::use_cases::deployment_start_candidate::{
    CandidateStartError, CandidateStartInput, start_candidate,
};
use crate::use_cases::deployment_transition::{TransitionDeploymentError, fail_deployment};

#[derive(Debug, PartialEq, Eq)]
pub struct DeploymentResult {
    pub deployment_id: String,
    pub runtime_id: String,
    pub container_name: String,
    pub image_reference: String,
    pub source_revision: Option<String>,
    pub finished_at: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PublicDeploymentConfiguration {
    pub managed_caddy_directory: PathBuf,
    pub caddyfile_path: PathBuf,
}

#[derive(Debug)]
pub enum DeployReleaseError {
    ApplicationNotFound {
        application_id: String,
    },
    PublicApplication {
        application_id: String,
    },
    LoadApplication {
        source: rusqlite::Error,
    },
    CreateDeployment {
        source: CreateDeploymentError,
    },
    DeploymentFailed {
        deployment_id: String,
        code: &'static str,
        source: Box<dyn Error>,
    },
    RecordFailure {
        deployment_id: String,
        failure: String,
        source: TransitionDeploymentError,
    },
    Cleanup {
        deployment_id: String,
        failure: String,
        source: Box<CandidateCleanupError>,
    },
}

impl fmt::Display for DeployReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::PublicApplication { application_id } => write!(
                formatter,
                "application `{application_id}` requires public deployment support"
            ),
            Self::LoadApplication { source } => {
                write!(
                    formatter,
                    "failed to load deployment specification: {source}"
                )
            }
            Self::CreateDeployment { source } => write!(formatter, "{source}"),
            Self::DeploymentFailed {
                deployment_id,
                code,
                source,
            } => write!(
                formatter,
                "deployment `{deployment_id}` failed with `{code}`: {source}"
            ),
            Self::RecordFailure {
                deployment_id,
                failure,
                source,
            } => write!(
                formatter,
                "deployment `{deployment_id}` encountered `{failure}` and its failure could not be recorded: {source}"
            ),
            Self::Cleanup {
                deployment_id,
                failure,
                source,
            } => write!(
                formatter,
                "deployment `{deployment_id}` encountered `{failure}` and its candidate could not be cleaned up: {source}"
            ),
        }
    }
}

impl Error for DeployReleaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::LoadApplication { source } => Some(source),
            Self::CreateDeployment { source } => Some(source),
            Self::DeploymentFailed { source, .. } => Some(source.as_ref()),
            Self::RecordFailure { source, .. } => Some(source),
            Self::Cleanup { source, .. } => Some(source.as_ref()),
            Self::ApplicationNotFound { .. } | Self::PublicApplication { .. } => None,
        }
    }
}

pub fn deploy_release(
    connection: &mut Connection,
    application_id: &str,
    release: &Release,
    deployment_type: DeploymentType,
    source_revision: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
) -> Result<DeploymentResult, DeployReleaseError> {
    let mut progress = ProgressReporter::disabled();
    deploy_release_reporting(
        connection,
        application_id,
        release,
        deployment_type,
        source_revision,
        public_configuration,
        &mut progress,
    )
}

pub fn deploy_release_with_progress(
    connection: &mut Connection,
    application_id: &str,
    release: &Release,
    deployment_type: DeploymentType,
    source_revision: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut dyn FnMut(DeploymentProgress),
) -> Result<DeploymentResult, DeployReleaseError> {
    let mut progress = ProgressReporter::enabled(progress);
    deploy_release_reporting(
        connection,
        application_id,
        release,
        deployment_type,
        source_revision,
        public_configuration,
        &mut progress,
    )
}

fn deploy_release_reporting(
    connection: &mut Connection,
    application_id: &str,
    release: &Release,
    deployment_type: DeploymentType,
    source_revision: Option<&str>,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeploymentResult, DeployReleaseError> {
    progress.started(
        DeploymentStep::LoadSpecification,
        format!("application {application_id}"),
    );
    let specification = load_specification(connection, application_id)?;
    progress.completed(
        DeploymentStep::LoadSpecification,
        format!(
            "application {}, visibility {}",
            specification.application_name,
            specification.visibility.database_value()
        ),
    );
    if specification.visibility == Visibility::Public && public_configuration.is_none() {
        return Err(DeployReleaseError::PublicApplication {
            application_id: application_id.to_owned(),
        });
    }

    progress.started(
        DeploymentStep::CreateDeployment,
        format!("release {}", release.id),
    );
    let deployment = create_deployment_with_source_revision(
        connection,
        application_id,
        &release.id,
        deployment_type,
        source_revision,
    )
    .map_err(|source| DeployReleaseError::CreateDeployment { source })?;
    progress.completed(
        DeploymentStep::CreateDeployment,
        format!("deployment {}", deployment.id),
    );
    progress.state_changed(&deployment.id, DeploymentStatus::Pending);

    let execution = execute_deployment(
        connection,
        &deployment.id,
        &specification,
        &release.image_reference,
        &release.image_digest,
        public_configuration,
        progress,
    );
    match execution {
        Ok((runtime_id, container_name, finished_at)) => Ok(DeploymentResult {
            deployment_id: deployment.id,
            runtime_id,
            container_name,
            image_reference: release.image_reference.clone(),
            source_revision: deployment.source_revision,
            finished_at,
        }),
        Err(failed) => finish_failed_deployment(connection, &deployment.id, failed, progress),
    }
}

struct DeploymentSpecification {
    application_id: String,
    application_name: String,
    container_port: u16,
    health_path: String,
    expected_status: u16,
    visibility: Visibility,
}

fn load_specification(
    connection: &Connection,
    application_id: &str,
) -> Result<DeploymentSpecification, DeployReleaseError> {
    let spec = match application_store::load_deployment_specification(connection, application_id) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return Err(DeployReleaseError::ApplicationNotFound {
                application_id: application_id.to_owned(),
            });
        }
        Err(ApplicationStoreError::NotFound { application_id }) => {
            return Err(DeployReleaseError::ApplicationNotFound { application_id });
        }
        Err(ApplicationStoreError::SystemNotFound { .. }) => {
            return Err(DeployReleaseError::LoadApplication {
                source: rusqlite::Error::QueryReturnedNoRows,
            });
        }
        Err(ApplicationStoreError::Persistence { source }) => {
            return Err(DeployReleaseError::LoadApplication { source });
        }
    };

    let visibility =
        Visibility::from_database(&spec.5).ok_or_else(|| DeployReleaseError::LoadApplication {
            source: rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid visibility: {}", spec.5),
                )),
            ),
        })?;

    Ok(DeploymentSpecification {
        application_id: spec.0,
        application_name: spec.1,
        container_port: spec.2,
        health_path: spec.3,
        expected_status: spec.4,
        visibility,
    })
}

struct FailedExecution {
    code: &'static str,
    source: Box<dyn Error>,
    failure_persisted: bool,
    resources: CandidateResources,
}

fn execute_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    specification: &DeploymentSpecification,
    image_reference: &str,
    artifact_identity: &str,
    public_configuration: Option<&PublicDeploymentConfiguration>,
    progress: &mut ProgressReporter<'_>,
) -> Result<(String, String, String), FailedExecution> {
    progress.state_changed(deployment_id, DeploymentStatus::Starting);
    progress.started(
        DeploymentStep::CreateContainer,
        format!("image {image_reference}"),
    );

    let input = CandidateStartInput {
        connection,
        deployment_id,
        application_id: &specification.application_id,
        application_name: &specification.application_name,
        image_reference,
        container_port: specification.container_port,
        artifact_identity,
    };

    let candidate = start_candidate(input).map_err(|err| match err {
        CandidateStartError::PortAllocation { source } => {
            failure_needing_persistence("runtime_port_allocation_failed", source, None, None)
        }
        CandidateStartError::UnitCreation { source, resources } => FailedExecution {
            code: "runtime_unit_creation_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::UnitReload { source, resources } => FailedExecution {
            code: "runtime_unit_reload_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::UnitStart { source, resources } => FailedExecution {
            code: "runtime_start_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::ContainerResolution { source, resources } => FailedExecution {
            code: "runtime_resolution_failed",
            source,
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::ContainerObservation { source, resources } => FailedExecution {
            code: "runtime_observation_failed",
            source,
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::RuntimeRegistration { source, resources } => FailedExecution {
            code: "runtime_registration_failed",
            source,
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::PortPersistence { source, resources } => FailedExecution {
            code: "runtime_port_persistence_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
        CandidateStartError::DeploymentTransition { source, resources } => FailedExecution {
            code: "deployment_transition_failed",
            source: Box::new(source),
            failure_persisted: false,
            resources: *resources,
        },
    })?;

    progress.completed(
        DeploymentStep::CreateContainer,
        format!(
            "unit {}, endpoint 127.0.0.1:{}",
            candidate.unit_name, candidate.port
        ),
    );
    progress.completed(
        DeploymentStep::StartContainer,
        format!("container {}", candidate.runtime.external_runtime_id),
    );
    progress.completed(
        DeploymentStep::ObserveContainer,
        format!("state Running, endpoint {}", candidate.runtime.endpoint),
    );
    progress.completed(
        DeploymentStep::RegisterCandidate,
        format!("runtime {}", candidate.runtime.id),
    );
    progress.state_changed(deployment_id, DeploymentStatus::Verifying);

    let previous_runtime = load_previous_runtime(
        connection,
        &specification.application_id,
        &candidate.runtime.id,
    )
    .map_err(|source| {
        candidate_failure(
            "runtime_reconciliation_failed",
            source,
            Some(&candidate.runtime.external_runtime_id),
            Some(&candidate.runtime.id),
            Some(&candidate.unit_name),
            true,
        )
    })?;

    if specification.visibility == Visibility::Public {
        let Some(public_configuration) = public_configuration else {
            return Err(failure_needing_persistence(
                "public_configuration_missing",
                DeployReleaseError::PublicApplication {
                    application_id: specification.application_id.clone(),
                },
                Some(&candidate.runtime.external_runtime_id),
                Some(&candidate.runtime.id),
            ));
        };
        let input = PublicActivationInput {
            connection,
            runtime: &candidate.runtime,
            application_id: &specification.application_id,
            health_path: &specification.health_path,
            expected_status: specification.expected_status,
            managed_caddy_directory: &public_configuration.managed_caddy_directory,
            caddyfile_path: &public_configuration.caddyfile_path,
        };
        let completed = activate_public_candidate(input, progress).map_err(|err| {
            let mut failed = match err {
                PublicActivationError::InternalHealth { source, resources } => FailedExecution {
                    code: "health_check_failed",
                    source,
                    failure_persisted: false,
                    resources: *resources,
                },
                PublicActivationError::DeploymentTransition { source, resources } => {
                    FailedExecution {
                        code: "deployment_transition_failed",
                        source: Box::new(source),
                        failure_persisted: false,
                        resources: *resources,
                    }
                }
                PublicActivationError::ExposurePreparation { source, resources } => {
                    FailedExecution {
                        code: "exposure_preparation_failed",
                        source,
                        failure_persisted: false,
                        resources: *resources,
                    }
                }
                PublicActivationError::CaddyMaterialization {
                    source, resources, ..
                } => FailedExecution {
                    code: "caddy_materialization_failed",
                    source,
                    failure_persisted: false,
                    resources: *resources,
                },
                PublicActivationError::ExternalHealth {
                    source, resources, ..
                } => FailedExecution {
                    code: "external_health_check_failed",
                    source,
                    failure_persisted: false,
                    resources: *resources,
                },
                PublicActivationError::PublicPromotion {
                    source, resources, ..
                } => FailedExecution {
                    code: "candidate_promotion_failed",
                    source,
                    failure_persisted: false,
                    resources: *resources,
                },
            };
            failed.resources = failed.resources.with_unit(&candidate.unit_name).with_port();
            failed
        });
        if completed.is_ok() {
            retire_previous_runtime(
                connection,
                &specification.application_name,
                previous_runtime.as_ref(),
            );
        }
        return completed.map(|output| {
            (
                candidate.runtime.id.clone(),
                candidate.container_name,
                output.finished_at,
            )
        });
    }

    progress.started(
        DeploymentStep::HealthCheckAndPromotion,
        format!(
            "runtime {}, path {}, expected status {}",
            candidate.runtime.id, specification.health_path, specification.expected_status
        ),
    );
    let promoted = promote_internal_candidate(
        connection,
        &candidate.runtime.id,
        &specification.health_path,
        specification.expected_status,
    )
    .map_err(|source| {
        let mut failed = if matches!(
            &source,
            PromoteInternalCandidateError::CandidateUnhealthy { .. }
        ) {
            failure_already_persisted(
                "health_check_failed",
                source,
                &candidate.runtime.external_runtime_id,
                &candidate.runtime.id,
            )
        } else {
            failure_needing_persistence(
                "candidate_promotion_failed",
                source,
                Some(&candidate.runtime.external_runtime_id),
                Some(&candidate.runtime.id),
            )
        };
        failed.resources = failed.resources.with_unit(&candidate.unit_name).with_port();
        failed
    })?;
    progress.completed(
        DeploymentStep::HealthCheckAndPromotion,
        format!("runtime {} promoted to Current", candidate.runtime.id),
    );
    progress.state_changed(deployment_id, DeploymentStatus::Succeeded);
    retire_previous_runtime(
        connection,
        &specification.application_name,
        previous_runtime.as_ref(),
    );

    Ok((
        candidate.runtime.id,
        candidate.container_name,
        promoted.finished_at,
    ))
}

fn finish_failed_deployment(
    connection: &mut Connection,
    deployment_id: &str,
    failed: FailedExecution,
    progress: &mut ProgressReporter<'_>,
) -> Result<DeploymentResult, DeployReleaseError> {
    let failure = failed.source.to_string();
    let record_error = if failed.failure_persisted {
        progress.failure_persisted(deployment_id, failed.code);
        None
    } else {
        match fail_deployment(connection, deployment_id, failed.code, &failure) {
            Ok(_) => {
                progress.failure_persisted(deployment_id, failed.code);
                None
            }
            Err(source) => Some(source),
        }
    };
    let cleanup_error = if failed.resources.container_id.is_some()
        || failed.resources.unit_name.is_some()
        || failed.resources.port_reserved
    {
        progress.started(
            DeploymentStep::CleanupCandidate,
            format!("deployment {deployment_id}"),
        );
        match cleanup_failed_candidate(
            connection,
            deployment_id,
            failed.resources.unit_name.as_deref(),
            failed.resources.container_id.as_deref(),
            failed.resources.runtime_id.as_deref(),
        ) {
            Ok(()) => {
                progress.completed(
                    DeploymentStep::CleanupCandidate,
                    format!("deployment {deployment_id}"),
                );
                None
            }
            Err(source) => Some(source),
        }
    } else {
        None
    };

    if let Some(source) = cleanup_error {
        return Err(DeployReleaseError::Cleanup {
            deployment_id: deployment_id.to_owned(),
            failure,
            source: Box::new(source),
        });
    }
    if let Some(source) = record_error {
        return Err(DeployReleaseError::RecordFailure {
            deployment_id: deployment_id.to_owned(),
            failure,
            source,
        });
    }

    Err(DeployReleaseError::DeploymentFailed {
        deployment_id: deployment_id.to_owned(),
        code: failed.code,
        source: failed.source,
    })
}

// Git, build, runtime, and ordinary promotion errors do not update the deployment
// themselves. Tag them as needing persistence so the common finalizer records the
// correct failure stage before performing any candidate cleanup.
fn failure_needing_persistence(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
) -> FailedExecution {
    let resources = match (container_id, runtime_id) {
        (Some(cid), Some(rid)) => CandidateResources::with_container_and_runtime(cid, rid),
        (Some(cid), None) => CandidateResources::with_container(cid),
        _ => CandidateResources::empty(),
    };
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: false,
        resources,
    }
}

fn candidate_failure(
    code: &'static str,
    source: impl Error + 'static,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
    unit_name: Option<&str>,
    port_reserved: bool,
) -> FailedExecution {
    let mut resources = match (container_id, runtime_id) {
        (Some(cid), Some(rid)) => CandidateResources::with_container_and_runtime(cid, rid),
        (Some(cid), None) => CandidateResources::with_container(cid),
        _ => CandidateResources::empty(),
    };
    if let Some(unit) = unit_name {
        resources = resources.with_unit(unit);
    }
    if port_reserved {
        resources = resources.with_port();
    }
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: false,
        resources,
    }
}

// An unhealthy-candidate result is different: promotion persists `Failed` before it
// returns the error. Tag it as already persisted so the finalizer removes the rejected
// candidate without trying to fail an already-terminal deployment a second time.
fn failure_already_persisted(
    code: &'static str,
    source: impl Error + 'static,
    container_id: &str,
    runtime_id: &str,
) -> FailedExecution {
    FailedExecution {
        code,
        source: Box::new(source),
        failure_persisted: true,
        resources: CandidateResources::with_container_and_runtime(container_id, runtime_id),
    }
}
