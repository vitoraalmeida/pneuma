use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::local_runtime::{ControlContainerError, remove_container};
use crate::adapters::port_allocator::{PortAllocationError, release_port};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::adapters::systemd_quadlet::{QuadletError, daemon_reload, remove_unit, stop, unit_name};
use crate::domain::identity::{ContainerId, RuntimeInstanceId};
use crate::domain::runtime::{PreviousRuntime, RuntimeState};

#[derive(Debug, Clone)]
// Tracks only resources proven to belong to a candidate for safe compensation.
pub(crate) struct CandidateResources {
    pub unit_name: Option<String>,
    pub container_id: Option<ContainerId>,
    pub runtime_id: Option<RuntimeInstanceId>,
    pub port_reserved: bool,
}

impl CandidateResources {
    // Starts an empty resource record before candidate effects begin.
    pub(crate) fn empty() -> Self {
        Self {
            unit_name: None,
            container_id: None,
            runtime_id: None,
            port_reserved: false,
        }
    }

    // Records a resolved container when no persisted runtime exists yet.
    pub(crate) fn with_container(container_id: &ContainerId) -> Self {
        Self {
            container_id: Some(container_id.clone()),
            ..Self::empty()
        }
    }

    // Records a resolved container and its registered runtime for later cleanup.
    pub(crate) fn with_container_and_runtime(
        container_id: &ContainerId,
        runtime_id: &RuntimeInstanceId,
    ) -> Self {
        Self {
            container_id: Some(container_id.clone()),
            runtime_id: Some(runtime_id.clone()),
            ..Self::empty()
        }
    }

    // Adds the generated unit whose removal is safe during compensation.
    pub(crate) fn with_unit(mut self, unit_name: &str) -> Self {
        self.unit_name = Some(unit_name.to_owned());
        self
    }

    // Marks that the deployment owns a port reservation that cleanup must release.
    pub(crate) fn with_port(mut self) -> Self {
        self.port_reserved = true;
        self
    }

    // Updates the tracked container after the runtime is resolved.
    pub(crate) fn with_container_mut(mut self, container_id: &ContainerId) -> Self {
        self.container_id = Some(container_id.clone());
        self
    }

    // Updates the tracked runtime after registration succeeds.
    pub(crate) fn with_runtime_mut(mut self, runtime_id: &RuntimeInstanceId) -> Self {
        self.runtime_id = Some(runtime_id.clone());
        self
    }
}

#[derive(Debug)]
pub enum CandidateCleanupError {
    StopUnit { source: QuadletError },
    RemoveUnit { source: QuadletError },
    ReloadUnits { source: QuadletError },
    RemoveContainer { source: ControlContainerError },
    ReleasePort { source: PortAllocationError },
    RuntimeStore { source: RuntimeStoreError },
    RuntimeChanged { runtime_id: RuntimeInstanceId },
    Persistence { source: rusqlite::Error },
}

impl fmt::Display for CandidateCleanupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RemoveContainer { source } => write!(formatter, "{source}"),
            Self::StopUnit { source }
            | Self::RemoveUnit { source }
            | Self::ReloadUnits { source } => write!(formatter, "{source}"),
            Self::ReleasePort { source } => write!(formatter, "{source}"),
            Self::RuntimeStore { source } => {
                write!(formatter, "failed to persist candidate removal: {source}")
            }
            Self::RuntimeChanged { runtime_id } => {
                write!(
                    formatter,
                    "runtime `{runtime_id}` changed while its candidate was being removed"
                )
            }
            Self::Persistence { source } => {
                write!(formatter, "failed to persist candidate removal: {source}")
            }
        }
    }
}

impl Error for CandidateCleanupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RemoveContainer { source } => Some(source),
            Self::StopUnit { source }
            | Self::RemoveUnit { source }
            | Self::ReloadUnits { source } => Some(source),
            Self::ReleasePort { source } => Some(source),
            Self::RuntimeStore { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::RuntimeChanged { .. } => None,
        }
    }
}

// Loads the predecessor retained during candidate activation for post-promotion retirement.
pub(crate) fn load_previous_runtime(
    connection: &Connection,
    application_id: &str,
    candidate_runtime_id: &str,
) -> Result<Option<PreviousRuntime>, RuntimeStoreError> {
    runtime_store::load_previous_runtime(connection, application_id, candidate_runtime_id)
}

// Retires the prior runtime only after promotion; failures remain warnings and do not undo it.
pub(crate) fn retire_previous_runtime(
    connection: &Connection,
    application_name: &str,
    previous: Option<&PreviousRuntime>,
) {
    // The Quadlet generator enables the unit for boot start itself by applying the
    // [Install] section of the .container file, so no `systemctl enable` is needed.
    let Some(previous) = previous else {
        return;
    };
    let previous_unit = unit_name(application_name, previous.deployment_id.as_str());
    let retirement = (|| -> Result<(), QuadletError> {
        stop(&previous_unit)?;
        remove_unit(&previous_unit)?;
        daemon_reload()?;
        Ok(())
    })();
    if let Err(source) = retirement {
        eprintln!(
            "warning: previous runtime {} could not be retired: {source}",
            previous.runtime_id
        );
        return;
    }
    if let Err(source) = remove_container(previous.external_runtime_id.as_str()) {
        eprintln!(
            "warning: previous runtime {} unit was retired but its container could not be removed: {source}",
            previous.runtime_id
        );
        return;
    }
    if !matches!(
        runtime_store::mark_runtime_removed(connection, previous.runtime_id.as_str()),
        Ok(crate::adapters::stores::PersistenceOutcome::Updated)
    ) {
        eprintln!(
            "warning: previous runtime {} was retired but could not be marked removed",
            previous.runtime_id
        );
    }
}

// Compensates failed candidates while refusing to remove a runtime that may already be active.
pub(crate) fn cleanup_failed_candidate(
    connection: &Connection,
    deployment_id: &str,
    unit: Option<&str>,
    container_id: Option<&ContainerId>,
    runtime_id: Option<&RuntimeInstanceId>,
) -> Result<(), CandidateCleanupError> {
    if let Some(runtime_id) = runtime_id {
        let state = runtime_store::load_runtime_state(connection, runtime_id.as_str())
            .map_err(|source| CandidateCleanupError::RuntimeStore { source })?;
        // A promotion error may have an uncertain external outcome. Never remove an
        // already active runtime.
        if state.is_some_and(|state| state != RuntimeState::Starting) {
            return Ok(());
        }
    }

    if let Some(unit) = unit {
        stop(unit).map_err(|source| CandidateCleanupError::StopUnit { source })?;
        remove_unit(unit).map_err(|source| CandidateCleanupError::RemoveUnit { source })?;
        daemon_reload().map_err(|source| CandidateCleanupError::ReloadUnits { source })?;
    }
    if let Some(container_id) = container_id {
        remove_container(container_id.as_str())
            .map_err(|source| CandidateCleanupError::RemoveContainer { source })?;
    }
    if let Some(runtime_id) = runtime_id {
        let outcome = runtime_store::mark_starting_runtime_missing(connection, runtime_id.as_str())
            .map_err(|source| CandidateCleanupError::RuntimeStore { source })?;
        if outcome == crate::adapters::stores::PersistenceOutcome::Stale {
            return Err(CandidateCleanupError::RuntimeChanged {
                runtime_id: runtime_id.clone(),
            });
        }
    }
    release_port(connection, deployment_id)
        .map_err(|source| CandidateCleanupError::ReleasePort { source })?;
    Ok(())
}
