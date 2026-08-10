use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::adapters::local_runtime::{ControlContainerError, remove_container};
use crate::adapters::port_allocator::{PortAllocationError, release_port};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::adapters::systemd_quadlet::{QuadletError, daemon_reload, remove_unit, stop, unit_name};

#[derive(Debug)]
pub enum CandidateCleanupError {
    StopUnit { source: QuadletError },
    RemoveUnit { source: QuadletError },
    ReloadUnits { source: QuadletError },
    RemoveContainer { source: ControlContainerError },
    ReleasePort { source: PortAllocationError },
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
            Self::Persistence { source } => Some(source),
        }
    }
}

pub(crate) struct PreviousRuntime {
    pub runtime_id: String,
    pub deployment_id: String,
    pub external_runtime_id: String,
}

pub(crate) fn load_previous_runtime(
    connection: &Connection,
    application_id: &str,
    candidate_runtime_id: &str,
) -> Result<Option<PreviousRuntime>, rusqlite::Error> {
    runtime_store::load_previous_runtime(connection, application_id, candidate_runtime_id)
        .map(|opt| {
            opt.map(
                |(runtime_id, deployment_id, external_runtime_id)| PreviousRuntime {
                    runtime_id,
                    deployment_id,
                    external_runtime_id,
                },
            )
        })
        .map_err(|e| match e {
            RuntimeStoreError::Persistence { source } => source,
            _ => rusqlite::Error::QueryReturnedNoRows,
        })
}

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
    let previous_unit = unit_name(application_name, &previous.deployment_id);
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
    if let Err(source) = remove_container(&previous.external_runtime_id) {
        eprintln!(
            "warning: previous runtime {} unit was retired but its container could not be removed: {source}",
            previous.runtime_id
        );
        return;
    }
    if let Err(source) = runtime_store::mark_runtime_removed(connection, &previous.runtime_id) {
        eprintln!(
            "warning: previous runtime {} was retired but could not be marked removed: {source}",
            previous.runtime_id
        );
    }
}

pub(crate) fn cleanup_failed_candidate(
    connection: &Connection,
    deployment_id: &str,
    unit: Option<&str>,
    container_id: Option<&str>,
    runtime_id: Option<&str>,
) -> Result<(), CandidateCleanupError> {
    if let Some(runtime_id) = runtime_id {
        let state =
            runtime_store::load_runtime_state(connection, runtime_id).map_err(|source| {
                CandidateCleanupError::Persistence {
                    source: match source {
                        RuntimeStoreError::Persistence { source } => source,
                        _ => rusqlite::Error::QueryReturnedNoRows,
                    },
                }
            })?;
        // A promotion error may have an uncertain external outcome. Never remove an
        // already active runtime.
        if state.as_deref().is_some_and(|state| state != "starting") {
            return Ok(());
        }
    }

    if let Some(unit) = unit {
        stop(unit).map_err(|source| CandidateCleanupError::StopUnit { source })?;
        remove_unit(unit).map_err(|source| CandidateCleanupError::RemoveUnit { source })?;
        daemon_reload().map_err(|source| CandidateCleanupError::ReloadUnits { source })?;
    }
    if let Some(container_id) = container_id {
        remove_container(container_id)
            .map_err(|source| CandidateCleanupError::RemoveContainer { source })?;
    }
    if let Some(runtime_id) = runtime_id {
        runtime_store::mark_starting_runtime_missing(connection, runtime_id).map_err(|source| {
            CandidateCleanupError::Persistence {
                source: match source {
                    RuntimeStoreError::Persistence { source } => source,
                    _ => rusqlite::Error::QueryReturnedNoRows,
                },
            }
        })?;
    }
    release_port(connection, deployment_id)
        .map_err(|source| CandidateCleanupError::ReleasePort { source })?;
    Ok(())
}
