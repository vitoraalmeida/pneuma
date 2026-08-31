use rusqlite::Connection;
use thiserror::Error;

use crate::adapters::local_runtime::{PodmanError, container_exists, remove_container};
use crate::adapters::port_allocator::{PortAllocationError, release_port};
use crate::adapters::stores::runtime_store;
use crate::adapters::systemd_quadlet::{QuadletError, daemon_reload, remove_unit, stop, unit_name};
use crate::domain::application::ApplicationName;
use crate::domain::identity::{ApplicationId, DeploymentId, RuntimeInstanceId};
use crate::domain::runtime::{ContainerId, PreviousRuntime, RuntimeState};

#[derive(Debug, Clone)]
// Tracks only resources proven to belong to a candidate for safe compensation.
pub(crate) struct CandidateResources {
    pub(crate) unit_name: Option<String>,
    pub(crate) container_id: Option<ContainerId>,
    pub(crate) runtime_id: Option<RuntimeInstanceId>,
    pub(crate) port_reserved: bool,
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

    // Reports whether any compensable resource is held so cleanup runs only when needed.
    pub(crate) fn needs_cleanup(&self) -> bool {
        self.container_id.is_some() || self.unit_name.is_some() || self.port_reserved
    }
}

#[derive(Debug, Error)]
pub enum CandidateCleanupError {
    #[error(transparent)]
    Supervision { source: QuadletError },
    #[error(transparent)]
    RemoveContainer { source: PodmanError },
    #[error(transparent)]
    ReleasePort { source: PortAllocationError },
    #[error("runtime `{runtime_id}` changed while its candidate was being removed")]
    RuntimeChanged { runtime_id: RuntimeInstanceId },
    #[error("container `{container_id}` remained present after its removal")]
    ContainerNotRemoved { container_id: String },
    #[error("failed to persist candidate removal: {source}")]
    Persistence {
        #[source]
        source: rusqlite::Error,
    },
}

impl From<rusqlite::Error> for CandidateCleanupError {
    fn from(source: rusqlite::Error) -> Self {
        Self::Persistence { source }
    }
}

// Loads the predecessor retained during candidate activation for post-promotion retirement.
pub(crate) fn load_previous_runtime(
    connection: &Connection,
    application_id: &ApplicationId,
    candidate_runtime_id: &RuntimeInstanceId,
) -> Result<Option<PreviousRuntime>, rusqlite::Error> {
    runtime_store::load_previous_runtime(connection, application_id, candidate_runtime_id)
}

// Retires the prior runtime only after promotion; every external destruction is proven
// by observation before retirement is recorded, and any failure remains a warning that
// does not undo the promotion.
pub(crate) fn retire_previous_runtime(
    connection: &Connection,
    application_name: &ApplicationName,
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
    if let Err(source) = prove_container_removed(&previous.external_runtime_id) {
        eprintln!(
            "warning: previous runtime {} unit was retired but its container removal could not be proven: {source}",
            previous.runtime_id
        );
        return;
    }
    if !matches!(
        runtime_store::mark_runtime_removed(connection, &previous.runtime_id),
        Ok(crate::adapters::stores::PersistenceOutcome::Updated)
    ) {
        eprintln!(
            "warning: previous runtime {} was retired but could not be marked removed",
            previous.runtime_id
        );
    }
}

// Proves a container is gone before any caller confirms its destruction: absence is
// observed first (Quadlet's ExecStop removes the container itself), a container still
// present is force-removed, and absence is re-observed so a removal that Podman did
// not actually apply is never treated as a completed destruction.
fn prove_container_removed(container_id: &ContainerId) -> Result<(), CandidateCleanupError> {
    if !container_exists(container_id)
        .map_err(|source| CandidateCleanupError::RemoveContainer { source })?
    {
        return Ok(());
    }
    remove_container(container_id.as_str())
        .map_err(|source| CandidateCleanupError::RemoveContainer { source })?;
    if container_exists(container_id)
        .map_err(|source| CandidateCleanupError::RemoveContainer { source })?
    {
        return Err(CandidateCleanupError::ContainerNotRemoved {
            container_id: container_id.to_string(),
        });
    }
    Ok(())
}

// Compensates failed candidates while refusing to remove a runtime that may already be active.
pub(crate) fn cleanup_failed_candidate(
    connection: &Connection,
    deployment_id: &DeploymentId,
    unit: Option<&str>,
    container_id: Option<&ContainerId>,
    runtime_id: Option<&RuntimeInstanceId>,
) -> Result<(), CandidateCleanupError> {
    if let Some(runtime_id) = runtime_id {
        let state = runtime_store::load_runtime_state(connection, runtime_id)?;
        // A promotion error may have an uncertain external outcome. Never remove an
        // already active runtime.
        if state.is_some_and(|state| state != RuntimeState::Starting) {
            return Ok(());
        }
    }

    if let Some(unit) = unit {
        stop(unit).map_err(|source| CandidateCleanupError::Supervision { source })?;
        remove_unit(unit).map_err(|source| CandidateCleanupError::Supervision { source })?;
        daemon_reload().map_err(|source| CandidateCleanupError::Supervision { source })?;
    }
    if let Some(container_id) = container_id {
        prove_container_removed(container_id)?;
    }
    if let Some(runtime_id) = runtime_id {
        let outcome = runtime_store::mark_starting_runtime_missing(connection, runtime_id)?;
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

#[cfg(test)]
mod tests {
    use super::{CandidateResources, ContainerId, RuntimeInstanceId};

    #[test]
    fn empty_resources_need_no_cleanup() {
        assert!(!CandidateResources::empty().needs_cleanup());
    }

    #[test]
    fn any_held_resource_needs_cleanup() {
        assert!(CandidateResources::empty().with_port().needs_cleanup());
        assert!(
            CandidateResources::empty()
                .with_unit("unit")
                .needs_cleanup()
        );
        assert!(
            CandidateResources::with_container_and_runtime(&container_id(), &runtime_id())
                .needs_cleanup()
        );
    }

    fn container_id() -> ContainerId {
        ContainerId::from("abc123")
    }

    fn runtime_id() -> RuntimeInstanceId {
        RuntimeInstanceId::new("11111111111111111111111111111111").unwrap()
    }
}
