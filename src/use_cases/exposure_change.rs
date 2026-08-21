use std::error::Error;
use std::fmt;
use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};

use crate::adapters::caddy_exposure::{
    CaddyRecoveryError, MaterializeCaddyFragmentError, canonical_fragment_contents,
    materialize_caddy_fragment, remove_caddy_fragment, restore_materialized_caddy_fragment,
    restore_removed_caddy_fragment,
};
use crate::adapters::health_check_external::{ExternalHealthCheckError, check_external_health};
use crate::adapters::local_runtime::{ObserveContainerError, observe_container};
use crate::adapters::stores::application_store;
use crate::adapters::stores::exposure_store::{self, ExposureStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::domain::exposure::{
    DomainName, Exposure, ExposureConfigurationVersion, ExposureDiagnostic,
    ExposureMaterializationState, Visibility,
};
use crate::domain::identity::ApplicationId;
use crate::domain::runtime::{ContainerObservation, ObservedRuntimeState};

#[derive(Debug, PartialEq, Eq)]
// Returns the requested visibility after its materialization outcome is confirmed.
pub struct ExposureChange {
    pub application_id: ApplicationId,
    pub visibility: Visibility,
    pub domain: Option<DomainName>,
}

#[derive(Debug)]
pub enum ExposureChangeError {
    ApplicationNotFound {
        application_id: String,
    },
    NoActiveRuntime {
        application_id: String,
    },
    DomainRequired {
        application_id: String,
    },
    ExposureChanged {
        application_id: String,
    },
    InvalidVisibility {
        visibility: String,
    },
    InvalidMaterializationState {
        state: String,
    },
    InvalidExposure {
        reason: String,
    },
    InvalidConfigurationVersion,
    InvalidDiagnostic,
    Store {
        source: ExposureStoreError,
    },
    RuntimeStore {
        source: RuntimeStoreError,
    },
    ApplicationStore {
        source: crate::adapters::stores::application_store::ApplicationStoreError,
    },
    ObserveFailed {
        source: ObserveContainerError,
    },
    RuntimeNotRunning {
        state: ObservedRuntimeState,
    },
    MaterializeFailed {
        source: MaterializeCaddyFragmentError,
    },
    RemoveFragmentFailed {
        source: CaddyRecoveryError,
    },
    ExternalHealthFailed {
        source: ExternalHealthCheckError,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for ExposureChangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationNotFound { application_id } => {
                write!(formatter, "application `{application_id}` was not found")
            }
            Self::NoActiveRuntime { application_id } => {
                write!(
                    formatter,
                    "application `{application_id}` has no active runtime to expose"
                )
            }
            Self::DomainRequired { application_id } => write!(
                formatter,
                "application `{application_id}` requires a domain for public exposure"
            ),
            Self::ExposureChanged { application_id } => write!(
                formatter,
                "exposure of application `{application_id}` changed while it was being materialized"
            ),
            Self::InvalidVisibility { visibility } => {
                write!(
                    formatter,
                    "application has invalid persisted visibility `{visibility}`"
                )
            }
            Self::InvalidMaterializationState { state } => {
                write!(
                    formatter,
                    "application has invalid persisted exposure materialization state `{state}`"
                )
            }
            Self::InvalidExposure { reason } => {
                write!(
                    formatter,
                    "application has invalid persisted exposure: {reason}"
                )
            }
            Self::InvalidConfigurationVersion => {
                formatter.write_str("generated exposure configuration version is invalid")
            }
            Self::InvalidDiagnostic => formatter
                .write_str("exposure diagnostic code and message must be trimmed and non-empty"),
            Self::Store { source } => write!(formatter, "failed to change exposure: {source}"),
            Self::RuntimeStore { source } => {
                write!(formatter, "failed to read runtime: {source}")
            }
            Self::ApplicationStore { source } => {
                write!(
                    formatter,
                    "failed to read application specification: {source}"
                )
            }
            Self::ObserveFailed { source } => {
                write!(formatter, "failed to observe runtime: {source}")
            }
            Self::RuntimeNotRunning { state } => {
                write!(formatter, "runtime is not running (state: {state:?})")
            }
            Self::MaterializeFailed { source } => {
                write!(formatter, "failed to materialize Caddy fragment: {source}")
            }
            Self::RemoveFragmentFailed { source } => {
                write!(formatter, "failed to remove Caddy fragment: {source}")
            }
            Self::ExternalHealthFailed { source } => {
                write!(formatter, "external health check failed: {source}")
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to persist exposure change: {source}")
            }
        }
    }
}

impl Error for ExposureChangeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store { source } => Some(source),
            Self::RuntimeStore { source } => Some(source),
            Self::ApplicationStore { source } => Some(source),
            Self::ObserveFailed { source } => Some(source),
            Self::MaterializeFailed { source } => Some(source),
            Self::RemoveFragmentFailed { source } => Some(source),
            Self::ExternalHealthFailed { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::ApplicationNotFound { .. }
            | Self::NoActiveRuntime { .. }
            | Self::DomainRequired { .. }
            | Self::ExposureChanged { .. }
            | Self::InvalidVisibility { .. }
            | Self::InvalidMaterializationState { .. }
            | Self::InvalidExposure { .. }
            | Self::InvalidConfigurationVersion
            | Self::InvalidDiagnostic
            | Self::RuntimeNotRunning { .. } => None,
        }
    }
}

// Orchestrates visibility intent, external Caddy effects, and short confirmation transactions.
pub fn change_exposure(
    connection: &mut Connection,
    application_id: &str,
    visibility: Visibility,
    managed_directory: &Path,
    caddyfile_path: &Path,
) -> Result<ExposureChange, ExposureChangeError> {
    let exposure =
        match exposure_store::load_exposure(connection, &ApplicationId::from(application_id)) {
            Ok(Some(exposure)) => exposure,
            Ok(None) => {
                return Err(ExposureChangeError::ApplicationNotFound {
                    application_id: application_id.to_owned(),
                });
            }
            Err(ExposureStoreError::InvalidVisibility { visibility, .. }) => {
                return Err(ExposureChangeError::InvalidVisibility { visibility });
            }
            Err(ExposureStoreError::InvalidMaterializationState { state, .. }) => {
                return Err(ExposureChangeError::InvalidMaterializationState { state });
            }
            Err(ExposureStoreError::InvalidExposure { reason, .. }) => {
                return Err(ExposureChangeError::InvalidExposure { reason });
            }
            Err(ExposureStoreError::Persistence { source }) => {
                return Err(ExposureChangeError::Persistence { source });
            }
        };
    if exposure.intent().visibility() == visibility {
        return Ok(ExposureChange {
            application_id: ApplicationId::from(application_id),
            visibility,
            domain: exposure.intent().domain().cloned(),
        });
    }
    if visibility == Visibility::Public && exposure.intent().domain().is_none() {
        return Err(ExposureChangeError::DomainRequired {
            application_id: application_id.to_owned(),
        });
    }
    begin_change(
        connection,
        application_id,
        exposure.intent().visibility(),
        visibility,
    )?;
    match visibility {
        Visibility::Public => make_public(
            connection,
            application_id,
            exposure,
            managed_directory,
            caddyfile_path,
        ),
        Visibility::Internal => make_internal(
            connection,
            application_id,
            exposure.intent().domain().cloned(),
            managed_directory,
            caddyfile_path,
        ),
    }
}

// Persists applying or removing intent before any Caddy side effect begins.
fn begin_change(
    connection: &mut Connection,
    application_id: &str,
    current_visibility: Visibility,
    desired_visibility: Visibility,
) -> Result<(), ExposureChangeError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| ExposureChangeError::Persistence { source })?;
    let updated = exposure_store::begin_exposure_change(
        &transaction,
        &ApplicationId::from(application_id),
        current_visibility,
        desired_visibility,
    )
    .map_err(|source| ExposureChangeError::Store { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(ExposureChangeError::ExposureChanged {
            application_id: application_id.to_owned(),
        });
    }
    transaction
        .commit()
        .map_err(|source| ExposureChangeError::Persistence { source })
}

// Publishes only an observed running runtime, compensating Caddy changes on later failure.
fn make_public(
    connection: &mut Connection,
    application_id: &str,
    exposure: Exposure,
    managed_directory: &Path,
    caddyfile_path: &Path,
) -> Result<ExposureChange, ExposureChangeError> {
    let domain = match exposure.intent().domain().cloned() {
        Some(domain) => domain,
        None => {
            return fail_public(
                connection,
                application_id,
                "domain_required",
                "public exposure requires a domain",
                false,
                ExposureChangeError::DomainRequired {
                    application_id: application_id.to_owned(),
                },
            );
        }
    };
    let Some(runtime) = runtime_store::load_current_successful_runtime(
        connection,
        &ApplicationId::from(application_id),
    )
    .map_err(|source| ExposureChangeError::RuntimeStore { source })?
    else {
        return fail_public(
            connection,
            application_id,
            "runtime_missing",
            "public exposure requires an active runtime",
            false,
            ExposureChangeError::NoActiveRuntime {
                application_id: application_id.to_owned(),
            },
        );
    };
    let observation = match observe_container(
        runtime.external_runtime_id.as_str(),
        runtime.container_port.get(),
    ) {
        Ok(observation) => observation,
        Err(source) => {
            let message = source.to_string();
            return fail_public(
                connection,
                application_id,
                "runtime_observation_failed",
                &message,
                false,
                ExposureChangeError::ObserveFailed { source },
            );
        }
    };
    let endpoint = match observation {
        ContainerObservation::Running { observed_endpoint } => observed_endpoint,
        ContainerObservation::NotRunning { state } => {
            return fail_public(
                connection,
                application_id,
                "runtime_not_running",
                &format!("runtime state is {state:?}"),
                false,
                ExposureChangeError::RuntimeNotRunning { state },
            );
        }
    };
    let materialized = match materialize_caddy_fragment(
        managed_directory,
        caddyfile_path,
        application_id,
        domain.as_str(),
        endpoint,
    ) {
        Ok(materialized) => materialized,
        Err(source) => {
            let diverged = source.recovery_failed();
            let message = source.to_string();
            return fail_public(
                connection,
                application_id,
                "caddy_materialization_failed",
                &message,
                diverged,
                ExposureChangeError::MaterializeFailed { source },
            );
        }
    };
    let specification = application_store::load_deployment_specification(
        connection,
        &ApplicationId::from(application_id),
    )
    .map_err(|source| ExposureChangeError::ApplicationStore { source })?
    .ok_or_else(|| ExposureChangeError::InvalidExposure {
        reason: "missing deployment specification".to_owned(),
    })?;
    if let Err(source) = check_external_health(
        domain.as_str(),
        specification.runtime.health_check().path().as_str(),
        specification.runtime.health_check().expected_status().get(),
    ) {
        let recovery_failed =
            restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
        let message = source.to_string();
        return fail_public(
            connection,
            application_id,
            "external_health_check_failed",
            &message,
            recovery_failed,
            ExposureChangeError::ExternalHealthFailed { source },
        );
    }
    let configuration_version =
        ExposureConfigurationVersion::new(&canonical_fragment_contents(domain.as_str(), endpoint))
            .map_err(|_| ExposureChangeError::InvalidConfigurationVersion)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| ExposureChangeError::Persistence { source })?;
    let completion = exposure_store::complete_public_exposure_change(
        &transaction,
        &ApplicationId::from(application_id),
        &runtime.id,
        &configuration_version,
    );
    let completed = match completion {
        Ok(completed) => completed,
        Err(source) => {
            drop(transaction);
            let recovery_failed =
                restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
            return fail_public(
                connection,
                application_id,
                "exposure_persistence_failed",
                "failed to persist a materialized public exposure",
                recovery_failed,
                ExposureChangeError::Store { source },
            );
        }
    };
    if completed == crate::adapters::stores::PersistenceOutcome::Stale {
        drop(transaction);
        let recovery_failed =
            restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
        return fail_public(
            connection,
            application_id,
            "exposure_changed",
            "exposure changed while Caddy was being materialized",
            recovery_failed,
            ExposureChangeError::ExposureChanged {
                application_id: application_id.to_owned(),
            },
        );
    }
    if let Err(source) = transaction.commit() {
        let recovery_failed =
            restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
        return fail_public(
            connection,
            application_id,
            "exposure_persistence_failed",
            "failed to commit a materialized public exposure",
            recovery_failed,
            ExposureChangeError::Persistence { source },
        );
    }
    Ok(ExposureChange {
        application_id: ApplicationId::from(application_id),
        visibility: Visibility::Public,
        domain: Some(domain),
    })
}

// Removes the managed route without changing the application's loopback runtime.
fn make_internal(
    connection: &mut Connection,
    application_id: &str,
    domain: Option<DomainName>,
    managed_directory: &Path,
    caddyfile_path: &Path,
) -> Result<ExposureChange, ExposureChangeError> {
    let removed = match remove_caddy_fragment(managed_directory, application_id, caddyfile_path) {
        Ok(removed) => removed,
        Err(source) => {
            let message = source.to_string();
            let diverged = source.recovery_failed();
            return fail_internal(
                connection,
                application_id,
                "caddy_removal_failed",
                &message,
                diverged,
                ExposureChangeError::RemoveFragmentFailed { source },
            );
        }
    };
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| ExposureChangeError::Persistence { source })?;
    let completion = exposure_store::complete_internal_exposure_change(
        &transaction,
        &ApplicationId::from(application_id),
    );
    let completed = match completion {
        Ok(completed) => completed,
        Err(source) => {
            drop(transaction);
            let recovery_failed = restore_removed_caddy_fragment(&removed, caddyfile_path).is_err();
            return fail_internal(
                connection,
                application_id,
                "exposure_persistence_failed",
                "failed to persist a removed internal exposure",
                recovery_failed,
                ExposureChangeError::Store { source },
            );
        }
    };
    if completed == crate::adapters::stores::PersistenceOutcome::Stale {
        drop(transaction);
        let recovery_failed = restore_removed_caddy_fragment(&removed, caddyfile_path).is_err();
        return fail_internal(
            connection,
            application_id,
            "exposure_changed",
            "exposure changed while Caddy was being removed",
            recovery_failed,
            ExposureChangeError::ExposureChanged {
                application_id: application_id.to_owned(),
            },
        );
    }
    if let Err(source) = transaction.commit() {
        let recovery_failed = restore_removed_caddy_fragment(&removed, caddyfile_path).is_err();
        return fail_internal(
            connection,
            application_id,
            "exposure_persistence_failed",
            "failed to commit a removed internal exposure",
            recovery_failed,
            ExposureChangeError::Persistence { source },
        );
    }
    Ok(ExposureChange {
        application_id: ApplicationId::from(application_id),
        visibility: Visibility::Internal,
        domain,
    })
}

// Persists public-route failure diagnostics after attempting any required compensation.
fn fail_public<T>(
    connection: &mut Connection,
    application_id: &str,
    code: &str,
    message: &str,
    diverged: bool,
    error: ExposureChangeError,
) -> Result<T, ExposureChangeError> {
    record_failure(
        connection,
        application_id,
        Visibility::Public,
        code,
        message,
        diverged,
    )?;
    Err(error)
}

// Persists internal-route failure diagnostics after attempting any required compensation.
fn fail_internal<T>(
    connection: &mut Connection,
    application_id: &str,
    code: &str,
    message: &str,
    diverged: bool,
    error: ExposureChangeError,
) -> Result<T, ExposureChangeError> {
    record_failure(
        connection,
        application_id,
        Visibility::Internal,
        code,
        message,
        diverged,
    )?;
    Err(error)
}

// Confirms failed or diverged materialization with a compare-and-set transaction.
fn record_failure(
    connection: &mut Connection,
    application_id: &str,
    visibility: Visibility,
    code: &str,
    message: &str,
    diverged: bool,
) -> Result<(), ExposureChangeError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| ExposureChangeError::Persistence { source })?;
    let state = if diverged {
        ExposureMaterializationState::Diverged
    } else {
        ExposureMaterializationState::Failed
    };
    let diagnostic = ExposureDiagnostic::new(code, message)
        .map_err(|_| ExposureChangeError::InvalidDiagnostic)?;
    let updated = exposure_store::record_exposure_change_failure(
        &transaction,
        &ApplicationId::from(application_id),
        visibility,
        state,
        &diagnostic,
    )
    .map_err(|source| ExposureChangeError::Store { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(ExposureChangeError::ExposureChanged {
            application_id: application_id.to_owned(),
        });
    }
    transaction
        .commit()
        .map_err(|source| ExposureChangeError::Persistence { source })
}
