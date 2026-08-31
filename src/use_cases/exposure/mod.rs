//! Public exposure changes for one application, driven by [`change_exposure`]:
//!
//! ```text
//! begin_change (persist intent) → make_public | make_internal → confirm
//!                                     \→ record_failure (diagnose after compensation)
//! ```
//!
//! Caddy is the external effect; every failure path compensates it first, then
//! records a compare-and-set diagnostic before returning.

use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;

use crate::adapters::application_lock::{ApplicationLock, ApplicationLockError};
use crate::adapters::caddy_exposure::{
    CaddyRecoveryError, MaterializeCaddyFragmentError, canonical_fragment_contents,
    materialize_caddy_fragment, remove_caddy_fragment, restore_materialized_caddy_fragment,
    restore_removed_caddy_fragment,
};
use crate::adapters::health_check_external::{ExternalHealthCheckError, check_external_health};
use crate::adapters::local_runtime::{PodmanError, observe_container};
use crate::adapters::stores::application_store;
use crate::adapters::stores::exposure_store::{self, ExposureStoreError};
use crate::adapters::stores::runtime_store;
use crate::domain::deployment::DeploymentFailureCode;
use crate::domain::exposure::{
    DomainName, Exposure, ExposureConfigurationVersion, ExposureDiagnostic, ExposureIntent,
    ExposureMaterializationState, Visibility,
};
use crate::domain::identity::ApplicationId;
use crate::domain::runtime::{ContainerObservation, ExpectedRuntimeEndpoint, ObservedRuntimeState};

#[derive(Debug, PartialEq, Eq)]
// Returns the requested visibility after its materialization outcome is confirmed.
pub struct ExposureChange {
    pub application_id: ApplicationId,
    pub visibility: Visibility,
    pub domain: Option<DomainName>,
}

#[derive(Debug, Error)]
pub enum ExposureChangeError {
    #[error("failed to acquire application lock: {source}")]
    ApplicationLock {
        #[source]
        source: ApplicationLockError,
    },
    #[error("application `{application_id}` already has an operation in progress")]
    ApplicationBusy { application_id: String },
    #[error("application `{application_id}` was not found")]
    ApplicationNotFound { application_id: String },
    #[error("application `{application_id}` has no active runtime to expose")]
    NoActiveRuntime { application_id: String },
    #[error("application `{application_id}` requires a domain for public exposure")]
    DomainRequired { application_id: String },
    #[error("exposure of application `{application_id}` changed while it was being materialized")]
    ExposureChanged { application_id: String },
    #[error("application has invalid persisted visibility `{visibility}`")]
    InvalidVisibility { visibility: String },
    #[error("application has invalid persisted exposure materialization state `{state}`")]
    InvalidMaterializationState { state: String },
    #[error("application has invalid persisted exposure: {reason}")]
    InvalidExposure { reason: String },
    #[error("generated exposure configuration version is invalid")]
    InvalidConfigurationVersion,
    #[error("exposure diagnostic code and message must be trimmed and non-empty")]
    InvalidDiagnostic,
    #[error("failed to change exposure: {source}")]
    Store {
        #[source]
        source: ExposureStoreError,
    },
    #[error("failed to read runtime: {source}")]
    RuntimeStore {
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to read application specification: {source}")]
    ApplicationStore {
        #[source]
        source: crate::adapters::stores::application_store::ApplicationStoreError,
    },
    #[error("failed to observe runtime: {source}")]
    ObserveFailed {
        #[source]
        source: PodmanError,
    },
    #[error("runtime is not running (state: {state:?})")]
    RuntimeNotRunning { state: ObservedRuntimeState },
    #[error("runtime `{container_id}` observed a non-loopback endpoint")]
    InvalidObservedEndpoint { container_id: String },
    #[error("failed to materialize Caddy fragment: {source}")]
    MaterializeFailed {
        #[source]
        source: MaterializeCaddyFragmentError,
    },
    #[error("failed to remove Caddy fragment: {source}")]
    RemoveFragmentFailed {
        #[source]
        source: CaddyRecoveryError,
    },
    #[error("external health check failed: {source}")]
    ExternalHealthFailed {
        #[source]
        source: ExternalHealthCheckError,
    },
    #[error("failed to persist exposure change: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

// Orchestrates visibility intent, external Caddy effects, and short confirmation transactions.
pub fn change_exposure(
    connection: &mut Connection,
    application_id: &ApplicationId,
    visibility: Visibility,
    managed_directory: &Path,
    caddyfile_path: &Path,
) -> Result<ExposureChange, ExposureChangeError> {
    let Some(_lock) = ApplicationLock::try_acquire_for_connection(connection, application_id)
        .map_err(|source| ExposureChangeError::ApplicationLock { source })?
    else {
        return Err(ExposureChangeError::ApplicationBusy {
            application_id: application_id.to_string(),
        });
    };
    let exposure = match exposure_store::load_exposure(connection, application_id) {
        Ok(Some(exposure)) => exposure,
        Ok(None) => {
            return Err(ExposureChangeError::ApplicationNotFound {
                application_id: application_id.to_string(),
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
            application_id: application_id.clone(),
            visibility,
            domain: exposure.intent().domain().cloned(),
        });
    }
    if visibility == Visibility::Public && exposure.intent().domain().is_none() {
        return Err(ExposureChangeError::DomainRequired {
            application_id: application_id.to_string(),
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
    application_id: &ApplicationId,
    expected_visibility: Visibility,
    desired_visibility: Visibility,
) -> Result<(), ExposureChangeError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| ExposureChangeError::Persistence { source })?;
    let updated = exposure_store::begin_exposure_change(
        &transaction,
        application_id,
        expected_visibility,
        desired_visibility,
    )
    .map_err(|source| ExposureChangeError::Store { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(ExposureChangeError::ExposureChanged {
            application_id: application_id.to_string(),
        });
    }
    transaction
        .commit()
        .map_err(|source| ExposureChangeError::Persistence { source })
}

// Publishes only an observed running runtime, compensating Caddy changes on later failure.
fn make_public(
    connection: &mut Connection,
    application_id: &ApplicationId,
    exposure: Exposure,
    managed_directory: &Path,
    caddyfile_path: &Path,
) -> Result<ExposureChange, ExposureChangeError> {
    let domain = match ExposureIntent::new(Visibility::Public, exposure.intent().domain().cloned())
    {
        Ok(ExposureIntent::Public { domain }) => domain,
        Ok(ExposureIntent::Internal { .. }) => {
            return fail_public(
                connection,
                application_id,
                "domain_required",
                "public exposure requires a domain",
                false,
                ExposureChangeError::DomainRequired {
                    application_id: application_id.to_string(),
                },
            );
        }
        Err(_) => {
            return fail_public(
                connection,
                application_id,
                "domain_required",
                "public exposure requires a domain",
                false,
                ExposureChangeError::DomainRequired {
                    application_id: application_id.to_string(),
                },
            );
        }
    };
    let Some(runtime) = runtime_store::load_active_successful_runtime(connection, application_id)
        .map_err(|source| ExposureChangeError::RuntimeStore { source })?
    else {
        return fail_public(
            connection,
            application_id,
            "runtime_missing",
            "public exposure requires an active runtime",
            false,
            ExposureChangeError::NoActiveRuntime {
                application_id: application_id.to_string(),
            },
        );
    };
    let observation = match observe_container(&runtime.external_runtime_id, runtime.container_port)
    {
        Ok(observation) => observation,
        Err(source) => {
            let message = source.to_string();
            return fail_public(
                connection,
                application_id,
                DeploymentFailureCode::RuntimeObservation.as_str(),
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
    let endpoint = ExpectedRuntimeEndpoint::new(endpoint).map_err(|_| {
        ExposureChangeError::InvalidObservedEndpoint {
            container_id: runtime.external_runtime_id.to_string(),
        }
    })?;
    let materialized = match materialize_caddy_fragment(
        managed_directory,
        caddyfile_path,
        application_id,
        &domain,
        endpoint,
    ) {
        Ok(materialized) => materialized,
        Err(source) => {
            let diverged = source.recovery_failed();
            let message = source.to_string();
            return fail_public(
                connection,
                application_id,
                DeploymentFailureCode::CaddyMaterialization.as_str(),
                &message,
                diverged,
                ExposureChangeError::MaterializeFailed { source },
            );
        }
    };
    let specification =
        application_store::load_deployment_specification(connection, application_id)
            .map_err(|source| ExposureChangeError::ApplicationStore { source })?
            .ok_or_else(|| ExposureChangeError::InvalidExposure {
                reason: "missing deployment specification".to_owned(),
            })?;
    if let Err(source) = check_external_health(
        &domain,
        specification.runtime.health_check().path(),
        specification.runtime.health_check().expected_status(),
    ) {
        let recovery_failed =
            restore_materialized_caddy_fragment(&materialized, caddyfile_path).is_err();
        let message = source.to_string();
        return fail_public(
            connection,
            application_id,
            DeploymentFailureCode::ExternalHealthCheck.as_str(),
            &message,
            recovery_failed,
            ExposureChangeError::ExternalHealthFailed { source },
        );
    }
    let configuration_version =
        ExposureConfigurationVersion::new(&canonical_fragment_contents(&domain, endpoint))
            .map_err(|_| ExposureChangeError::InvalidConfigurationVersion)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| ExposureChangeError::Persistence { source })?;
    let completion = exposure_store::complete_public_exposure_change(
        &transaction,
        application_id,
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
                application_id: application_id.to_string(),
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
        application_id: application_id.clone(),
        visibility: Visibility::Public,
        domain: Some(domain),
    })
}

// Persists public-route failure diagnostics after attempting any required compensation.
fn fail_public<T>(
    connection: &mut Connection,
    application_id: &ApplicationId,
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

// Confirms failed or diverged materialization with a compare-and-set transaction.
fn record_failure(
    connection: &mut Connection,
    application_id: &ApplicationId,
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
        application_id,
        visibility,
        state,
        &diagnostic,
    )
    .map_err(|source| ExposureChangeError::Store { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(ExposureChangeError::ExposureChanged {
            application_id: application_id.to_string(),
        });
    }
    transaction
        .commit()
        .map_err(|source| ExposureChangeError::Persistence { source })
}

// Removes the managed route without changing the application's loopback runtime.
fn make_internal(
    connection: &mut Connection,
    application_id: &ApplicationId,
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
    let completion =
        exposure_store::complete_internal_exposure_change(&transaction, application_id);
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
                application_id: application_id.to_string(),
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
        application_id: application_id.clone(),
        visibility: Visibility::Internal,
        domain,
    })
}

// Persists internal-route failure diagnostics after attempting any required compensation.
fn fail_internal<T>(
    connection: &mut Connection,
    application_id: &ApplicationId,
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
