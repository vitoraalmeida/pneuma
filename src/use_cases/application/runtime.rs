use std::error::Error;
use std::fmt;
use std::net::SocketAddr;

use rusqlite::Connection;

use crate::adapters::local_runtime::{
    ContainerCommandOutput, PodmanError, observe_container, resolve_container_id, start_container,
    stop_container,
};
use crate::adapters::stores::application_store::{self, ApplicationStoreError};
use crate::adapters::stores::runtime_store::{self, RuntimeStoreError};
use crate::adapters::systemd_quadlet::{
    QuadletError, container_name, start as start_unit, stop as stop_unit, unit_exists, unit_name,
};
use crate::domain::application::{ApplicationName, DesiredRuntimeState};
use crate::domain::identity::{ApplicationId, RuntimeInstanceId};
use crate::domain::runtime::{
    ContainerId, ContainerObservation, ObservedRuntimeState, RuntimeInstance,
};

#[derive(Debug, PartialEq, Eq)]
// Combines persisted operator intent with the latest observed runtime state for status commands.
pub struct RuntimeObservation {
    pub desired_runtime_state: DesiredRuntimeState,
    pub observed_runtime_state: ObservedRuntimeState,
    pub runtime_id: RuntimeInstanceId,
    pub container_id: ContainerId,
    pub observed_endpoint: Option<SocketAddr>,
}

impl RuntimeObservation {
    // Builds the operator-facing snapshot from a runtime record and its latest
    // external observation; a missing observation yields no endpoint by rule.
    fn recorded(
        desired_runtime_state: DesiredRuntimeState,
        runtime_id: RuntimeInstanceId,
        container_id: ContainerId,
        observation: &ContainerObservation,
    ) -> Self {
        Self {
            desired_runtime_state,
            observed_runtime_state: observation.state().clone(),
            runtime_id,
            container_id,
            observed_endpoint: observation.observed_endpoint(),
        }
    }
}

// The two operator commands a runtime transition can apply. Each variant owns every
// aspect of the command: the persisted intent, the observation that ends it, the
// diagnostic label, and the direct-container fallback used without supervision.
#[derive(Clone, Copy)]
enum RuntimeCommand {
    Start,
    Stop,
}

impl RuntimeCommand {
    // The operator intent persisted before any external effect is applied.
    fn desired_state(self) -> DesiredRuntimeState {
        match self {
            Self::Start => DesiredRuntimeState::Running,
            Self::Stop => DesiredRuntimeState::Stopped,
        }
    }

    // The observation that ends a successful transition.
    fn target_observation(self) -> ObservedRuntimeState {
        match self {
            Self::Start => ObservedRuntimeState::Running,
            Self::Stop => ObservedRuntimeState::Stopped,
        }
    }

    // Labels external effects in supervision and control diagnostics.
    fn operation(self) -> &'static str {
        match self {
            Self::Start => "starting",
            Self::Stop => "stopping",
        }
    }

    // Fallback lifecycle command applied straight to the container when the
    // managed unit does not exist.
    fn direct_control(self) -> fn(&str) -> Result<ContainerCommandOutput, PodmanError> {
        match self {
            Self::Start => start_container,
            Self::Stop => stop_container,
        }
    }
}

#[derive(Debug)]
pub enum RuntimeLifecycleError {
    NotDeployed {
        application_name: String,
    },
    ContainerMissing {
        application_name: String,
    },
    RuntimeChanged {
        runtime_id: String,
    },
    InvalidDesiredState {
        state: String,
    },
    Store {
        source: RuntimeStoreError,
    },
    ApplicationStore {
        source: ApplicationStoreError,
    },
    Observe {
        runtime_id: String,
        source: PodmanError,
    },
    Control {
        operation: &'static str,
        runtime_id: String,
        source: Box<PodmanError>,
    },
    Supervision {
        operation: &'static str,
        runtime_id: String,
        source: QuadletError,
    },
    Persistence {
        source: rusqlite::Error,
    },
}

impl fmt::Display for RuntimeLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotDeployed { application_name } => write!(
                formatter,
                "application `{application_name}` is not deployed"
            ),
            Self::ContainerMissing { application_name } => write!(
                formatter,
                "the container of application `{application_name}` is missing; run `pneuma app start` to recover it or `pneuma app deploy` to recreate it"
            ),
            Self::RuntimeChanged { runtime_id } => write!(
                formatter,
                "runtime `{runtime_id}` changed while it was being controlled"
            ),
            Self::InvalidDesiredState { state } => write!(
                formatter,
                "application has invalid persisted desired state `{state}`"
            ),
            Self::Store { source } => {
                write!(formatter, "failed to control application runtime: {source}")
            }
            Self::ApplicationStore { source } => {
                write!(formatter, "failed to control application runtime: {source}")
            }
            Self::Observe { runtime_id, source } => write!(
                formatter,
                "failed to observe runtime `{runtime_id}`: {source}"
            ),
            Self::Control {
                operation,
                runtime_id,
                source,
            } => write!(
                formatter,
                "failed while {operation} runtime `{runtime_id}`: {source}"
            ),
            Self::Supervision {
                operation,
                runtime_id,
                source,
            } => write!(
                formatter,
                "failed while {operation} supervised runtime `{runtime_id}`: {source}"
            ),
            Self::Persistence { source } => {
                write!(formatter, "failed to control application runtime: {source}")
            }
        }
    }
}

impl Error for RuntimeLifecycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Observe { source, .. } => Some(source),
            Self::Control { source, .. } => Some(source.as_ref()),
            Self::Supervision { source, .. } => Some(source),
            Self::Store { source } => Some(source),
            Self::ApplicationStore { source } => Some(source),
            Self::Persistence { source } => Some(source),
            Self::NotDeployed { .. }
            | Self::ContainerMissing { .. }
            | Self::RuntimeChanged { .. }
            | Self::InvalidDesiredState { .. } => None,
        }
    }
}

// Observes the current runtime and persists its state without changing the operator's intent.
pub fn report_application_status(
    connection: &mut Connection,
    application_id: &ApplicationId,
    application_name: &ApplicationName,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    let runtime = load_active_runtime(connection, application_id, application_name)?;
    let desired_runtime_state = load_desired_state(connection, application_id)?;
    let observation = observe_container(&runtime.external_runtime_id, runtime.container_port)
        .map_err(|source| RuntimeLifecycleError::Observe {
            runtime_id: runtime.id.to_string(),
            source,
        })?;
    if *observation.state() == ObservedRuntimeState::Missing
        && !missing_container_satisfies_stop_intent(&observation, desired_runtime_state)
    {
        persist_observation(connection, &runtime, &observation)?;
        return Err(RuntimeLifecycleError::ContainerMissing {
            application_name: application_name.as_str().to_owned(),
        });
    }
    persist_observation(connection, &runtime, &observation)?;

    Ok(RuntimeObservation::recorded(
        desired_runtime_state,
        runtime.id,
        runtime.external_runtime_id,
        &observation,
    ))
}

// Records stopped intent before delegating the runtime transition to the shared controller.
pub fn stop_application(
    connection: &mut Connection,
    application_id: &ApplicationId,
    application_name: &ApplicationName,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    transition_application(
        connection,
        application_id,
        application_name,
        RuntimeCommand::Stop,
    )
}

// Records running intent before delegating the runtime transition to the shared controller.
pub fn start_application(
    connection: &mut Connection,
    application_id: &ApplicationId,
    application_name: &ApplicationName,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    transition_application(
        connection,
        application_id,
        application_name,
        RuntimeCommand::Start,
    )
}

// Coordinates intent persistence, external control, and observation while preserving
// a stable runtime record across Quadlet container recreation.
fn transition_application(
    connection: &Connection,
    application_id: &ApplicationId,
    application_name: &ApplicationName,
    command: RuntimeCommand,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    let desired_runtime_state = command.desired_state();
    let runtime = load_active_runtime(connection, application_id, application_name)?;
    // The desired state is the operator's intent and is persisted before any external
    // effect, so an interrupted control operation still leaves the intent recorded.
    set_desired_state(connection, application_id, desired_runtime_state)?;

    let current = observe_active_runtime(connection, &runtime, application_name)?;
    if *current.observation.state() == ObservedRuntimeState::Missing {
        return handle_missing_runtime(connection, &runtime, application_name, command, current);
    }

    transition_observed_runtime(connection, &runtime, application_name, command, current)
}

// A container reported missing while the operator wants the application stopped
// is a stop already carried out (Quadlet removes the container on ExecStop).
// The observation is recorded without retiring the runtime so subsequent
// stop/start/status commands still find it.
fn missing_container_satisfies_stop_intent(
    observation: &ContainerObservation,
    desired_runtime_state: DesiredRuntimeState,
) -> bool {
    *observation.state() == ObservedRuntimeState::Missing
        && desired_runtime_state == DesiredRuntimeState::Stopped
}

// Interprets an absent container against the operator's intent: a stop is already
// satisfied, a start attempts supervised recovery through the stable Quadlet
// identity, and anything unresolved remains an explicit missing-container error.
fn handle_missing_runtime(
    connection: &Connection,
    runtime: &RuntimeInstance,
    application_name: &ApplicationName,
    command: RuntimeCommand,
    current: ActiveRuntimeObservation,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    let desired_runtime_state = command.desired_state();
    // Recording the missing observation without retiring the runtime keeps later
    // commands operable instead of reporting the application as undeployed.
    if missing_container_satisfies_stop_intent(&current.observation, desired_runtime_state) {
        persist_observation(connection, runtime, &current.observation)?;
        return Ok(RuntimeObservation::recorded(
            desired_runtime_state,
            runtime.id.clone(),
            current.container_id,
            &current.observation,
        ));
    }
    // Reaching this point means the operator wants the application running. When the
    // unit exists, starting it recreates the container under the stable name; observe
    // afresh to adopt the recreated identity.
    if supervised_unit_exists(application_name, runtime)? {
        start_unit(&unit_name(application_name, &runtime.deployment_id)).map_err(|source| {
            RuntimeLifecycleError::Supervision {
                operation: command.operation(),
                runtime_id: runtime.id.to_string(),
                source,
            }
        })?;
        let recovered = observe_active_runtime(connection, runtime, application_name)?;
        if *recovered.observation.state() != ObservedRuntimeState::Missing {
            persist_observation(connection, runtime, &recovered.observation)?;
            return Ok(RuntimeObservation::recorded(
                desired_runtime_state,
                runtime.id.clone(),
                recovered.container_id,
                &recovered.observation,
            ));
        }
    }
    persist_observation(connection, runtime, &current.observation)?;
    Err(RuntimeLifecycleError::ContainerMissing {
        application_name: application_name.as_str().to_owned(),
    })
}

// Controls an observed runtime toward the operator's intent and records the resulting
// observation. When a Quadlet stop removes the container (ExecStop), the resulting
// missing observation is recorded without retiring the runtime so later commands stay
// operable.
fn transition_observed_runtime(
    connection: &Connection,
    runtime: &RuntimeInstance,
    application_name: &ApplicationName,
    command: RuntimeCommand,
    current: ActiveRuntimeObservation,
) -> Result<RuntimeObservation, RuntimeLifecycleError> {
    let container_id = current.container_id;
    let observation = if *current.observation.state() == command.target_observation() {
        current.observation
    } else {
        apply_runtime_control(application_name, runtime, command, &container_id)?;
        observe_container(&container_id, runtime.container_port).map_err(|source| {
            RuntimeLifecycleError::Observe {
                runtime_id: runtime.id.to_string(),
                source,
            }
        })?
    };
    persist_observation(connection, runtime, &observation)?;

    Ok(RuntimeObservation::recorded(
        command.desired_state(),
        runtime.id.clone(),
        container_id,
        &observation,
    ))
}

// Prefers the managed systemd unit for runtime control and falls back to addressing
// the container directly only when the unit has not been materialized.
fn apply_runtime_control(
    application_name: &ApplicationName,
    runtime: &RuntimeInstance,
    command: RuntimeCommand,
    container_id: &ContainerId,
) -> Result<(), RuntimeLifecycleError> {
    let unit = unit_name(application_name, &runtime.deployment_id);
    if supervised_unit_exists(application_name, runtime)? {
        let result = match command {
            RuntimeCommand::Start => start_unit(&unit),
            RuntimeCommand::Stop => stop_unit(&unit),
        };
        return result.map_err(|source| RuntimeLifecycleError::Supervision {
            operation: command.operation(),
            runtime_id: runtime.id.to_string(),
            source,
        });
    }
    let control = command.direct_control();
    control(container_id.as_str()).map_err(|source| RuntimeLifecycleError::Control {
        operation: command.operation(),
        runtime_id: runtime.id.to_string(),
        source: Box::new(source),
    })?;
    Ok(())
}

// Reports whether the stable Quadlet unit for this runtime is materialized, mapping
// supervision failures into diagnostics tied to the runtime being controlled.
fn supervised_unit_exists(
    application_name: &ApplicationName,
    runtime: &RuntimeInstance,
) -> Result<bool, RuntimeLifecycleError> {
    let unit = unit_name(application_name, &runtime.deployment_id);
    unit_exists(&unit).map_err(|source| RuntimeLifecycleError::Supervision {
        operation: "checking Quadlet unit for",
        runtime_id: runtime.id.to_string(),
        source,
    })
}

// Quadlet recreates the container under the stable `pneuma-{application}-{deployment}`
// name with a fresh id whenever its unit restarts (for example, after a reboot). The
// persisted runtime identity can therefore go stale; reconcile it against the name
// before concluding the runtime is gone.
struct ActiveRuntimeObservation {
    observation: ContainerObservation,
    container_id: ContainerId,
}

fn observe_active_runtime(
    connection: &Connection,
    runtime: &RuntimeInstance,
    application_name: &ApplicationName,
) -> Result<ActiveRuntimeObservation, RuntimeLifecycleError> {
    let observation = observe_container(&runtime.external_runtime_id, runtime.container_port)
        .map_err(|source| RuntimeLifecycleError::Observe {
            runtime_id: runtime.id.to_string(),
            source,
        })?;
    if *observation.state() != ObservedRuntimeState::Missing {
        return Ok(ActiveRuntimeObservation {
            observation,
            container_id: runtime.external_runtime_id.clone(),
        });
    }
    let resolved =
        match resolve_container_id(&container_name(application_name, &runtime.deployment_id)) {
            Ok(id) => id,
            Err(_) => {
                return Ok(ActiveRuntimeObservation {
                    observation,
                    container_id: runtime.external_runtime_id.clone(),
                });
            }
        };
    let reconciled = runtime_store::reconcile_external_runtime_id(
        connection,
        &runtime.id,
        &runtime.external_runtime_id,
        &resolved,
    )
    .map_err(|source| RuntimeLifecycleError::Store { source })?;
    if reconciled == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: runtime.id.to_string(),
        });
    }
    let observation = observe_container(&resolved, runtime.container_port).map_err(|source| {
        RuntimeLifecycleError::Observe {
            runtime_id: runtime.id.to_string(),
            source,
        }
    })?;
    Ok(ActiveRuntimeObservation {
        observation,
        container_id: resolved,
    })
}

// Loads the active successful runtime, rejecting lifecycle commands for undeployed applications.
fn load_active_runtime(
    connection: &Connection,
    application_id: &ApplicationId,
    application_name: &ApplicationName,
) -> Result<RuntimeInstance, RuntimeLifecycleError> {
    runtime_store::load_active_successful_runtime(connection, application_id)
        .map_err(|source| RuntimeLifecycleError::Store { source })?
        .ok_or_else(|| RuntimeLifecycleError::NotDeployed {
            application_name: application_name.as_str().to_owned(),
        })
}

// Maps persisted desired state into the domain value and surfaces corrupt values explicitly.
fn load_desired_state(
    connection: &Connection,
    application_id: &ApplicationId,
) -> Result<DesiredRuntimeState, RuntimeLifecycleError> {
    match application_store::load_desired_runtime_state(connection, application_id) {
        Ok(state) => Ok(state),
        Err(ApplicationStoreError::InvalidDesiredRuntimeState { state, .. }) => {
            Err(RuntimeLifecycleError::InvalidDesiredState { state })
        }
        Err(source) => Err(RuntimeLifecycleError::ApplicationStore { source }),
    }
}

// Updates operator intent with compare-and-set semantics so concurrent changes are not lost.
fn set_desired_state(
    connection: &Connection,
    application_id: &ApplicationId,
    desired_runtime_state: DesiredRuntimeState,
) -> Result<(), RuntimeLifecycleError> {
    let expected = load_desired_state(connection, application_id)?;
    let updated = application_store::compare_and_set_desired_runtime_state(
        connection,
        application_id,
        expected,
        desired_runtime_state,
    )
    .map_err(|source| RuntimeLifecycleError::ApplicationStore { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: application_id.to_string(),
        });
    }
    Ok(())
}

// Persists an observation only while the runtime record remains current.
fn persist_observation(
    connection: &Connection,
    runtime: &RuntimeInstance,
    observation: &ContainerObservation,
) -> Result<(), RuntimeLifecycleError> {
    let updated = runtime_store::persist_observation(connection, &runtime.id, observation)
        .map_err(|source| RuntimeLifecycleError::Store { source })?;
    if updated == crate::adapters::stores::PersistenceOutcome::Stale {
        return Err(RuntimeLifecycleError::RuntimeChanged {
            runtime_id: runtime.id.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::adapters::database;
    use crate::adapters::systemd_quadlet::QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE;
    use crate::domain::identity::ApplicationId;
    use crate::test_support::{ScopedExternalPath, lock_quadlet_directory};

    const APPLICATION_ID: &str = "11111111111111111111111111111111";
    const DEPLOYMENT_ID: &str = "33333333333333333333333333333333";
    const RUNTIME_ID: &str = "44444444444444444444444444444444";
    const RECORDED_CONTAINER_ID: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const RECREATED_CONTAINER_ID: &str =
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    // Fake `podman` keyed by a shared state file naming the one container that
    // currently exists: an empty or absent file means nothing exists, so existence
    // checks and stable-name resolution fail exactly like Podman would before a
    // Quadlet start recreates the container. A direct stop empties the file,
    // mirroring the Quadlet ExecStop contract. Only shell builtins are used
    // because the scoped PATH exposes no coreutils.
    const FAKE_PODMAN: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_PODMAN_LOG\"
present=\"\"
if [ -f \"$PNEUMA_FAKE_PODMAN_STATE\" ]; then
  read present < \"$PNEUMA_FAKE_PODMAN_STATE\"
fi
if [ \"$1\" = \"container\" ] && [ \"$2\" = \"exists\" ]; then
  if [ -n \"$present\" ] && [ \"$3\" = \"$present\" ]; then
    exit 0
  fi
  exit 1
fi
if [ \"$1\" = \"stop\" ]; then
  : > \"$PNEUMA_FAKE_PODMAN_STATE\"
  exit 0
fi
if [ \"$1\" = \"port\" ]; then
  printf '%s\\n' \"${PNEUMA_FAKE_PODMAN_PORT:-127.0.0.1:31000}\"
  exit 0
fi
if [ \"$1\" = \"inspect\" ]; then
  case \"$3\" in
    \"{{.Id}}\")
      if [ -n \"$present\" ]; then
        printf '%s\\n' \"$present\"
        exit 0
      fi
      exit 1;;
    \"{{.State.Status}}\")
      printf '%s\\n' \"${PNEUMA_FAKE_PODMAN_STATUS:-running}\";;
    *)
      printf '\\n';;
  esac
  exit \"${PNEUMA_FAKE_PODMAN_INSPECT_EXIT:-0}\"
fi
exit \"${PNEUMA_FAKE_PODMAN_EXIT:-0}\"
";

    // Fake `systemctl` whose start materializes the recreated container id and
    // whose stop removes the container, mirroring how Quadlet manages the
    // lifecycle of the generated user service.
    const FAKE_SYSTEMCTL: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_SYSTEMCTL_LOG\"
case \"$2\" in
  start)
    printf '%s\\n' \"$PNEUMA_FAKE_SYSTEMCTL_CREATES\" > \"$PNEUMA_FAKE_PODMAN_STATE\";;
  stop)
    : > \"$PNEUMA_FAKE_PODMAN_STATE\";;
esac
exit \"${PNEUMA_FAKE_SYSTEMCTL_EXIT:-0}\"
";

    // Scopes PATH fakes, the Quadlet directory, and all behavior variables of one
    // scenario; logs stay readable for asserting which tool controlled the runtime.
    struct RuntimeScenario {
        path: ScopedExternalPath,
        _quadlet_lock: std::sync::MutexGuard<'static, ()>,
        previous_quadlet_directory: Option<OsString>,
        quadlet_directory: PathBuf,
        podman_log: PathBuf,
        systemctl_log: PathBuf,
        container_state: PathBuf,
    }

    impl RuntimeScenario {
        fn new(name: &str) -> Self {
            let path = ScopedExternalPath::new(
                name,
                &[("podman", FAKE_PODMAN), ("systemctl", FAKE_SYSTEMCTL)],
            );
            let _quadlet_lock = lock_quadlet_directory();
            for variable in [
                "PNEUMA_FAKE_PODMAN_STATUS",
                "PNEUMA_FAKE_PODMAN_PORT",
                "PNEUMA_FAKE_PODMAN_INSPECT_EXIT",
                "PNEUMA_FAKE_PODMAN_EXIT",
                "PNEUMA_FAKE_SYSTEMCTL_EXIT",
                "PNEUMA_FAKE_SYSTEMCTL_CREATES",
            ] {
                path.remove_var(variable);
            }
            let quadlet_directory = path.directory().join("quadlet");
            let previous_quadlet_directory =
                std::env::var_os(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE);
            path.set_var(
                QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE,
                &quadlet_directory.to_string_lossy(),
            );
            let podman_log = path.directory().join("podman.log");
            let systemctl_log = path.directory().join("systemctl.log");
            let container_state = path.directory().join("container.state");
            path.set_var("PNEUMA_FAKE_PODMAN_LOG", &podman_log.to_string_lossy());
            path.set_var(
                "PNEUMA_FAKE_SYSTEMCTL_LOG",
                &systemctl_log.to_string_lossy(),
            );
            path.set_var(
                "PNEUMA_FAKE_PODMAN_STATE",
                &container_state.to_string_lossy(),
            );
            Self {
                path,
                _quadlet_lock,
                previous_quadlet_directory,
                quadlet_directory,
                podman_log,
                systemctl_log,
                container_state,
            }
        }

        fn set_var(&self, name: &str, value: &str) {
            self.path.set_var(name, value);
        }

        // Emulates the container recorded by the runtime being alive before the
        // scenario's command runs.
        fn seed_container_present(&self, id: &str) {
            fs::write(&self.container_state, format!("{id}\n")).unwrap();
        }

        // Materializes the stable Quadlet unit so supervision is preferred.
        fn install_unit(&self, unit: &str) {
            fs::create_dir_all(&self.quadlet_directory).unwrap();
            fs::write(self.quadlet_directory.join(format!("{unit}.container")), "").unwrap();
        }

        fn invocations(&self, log: &Path) -> Vec<String> {
            fs::read_to_string(log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        fn podman_invocations(&self) -> Vec<String> {
            self.invocations(&self.podman_log)
        }

        fn systemctl_invocations(&self) -> Vec<String> {
            self.invocations(&self.systemctl_log)
        }
    }

    impl Drop for RuntimeScenario {
        fn drop(&mut self) {
            match self.previous_quadlet_directory.take() {
                Some(previous) => {
                    // Safety: PNEUMA_QUADLET_DIR writes are serialized by the
                    // quadlet-directory lock, which `_quadlet_lock` still holds.
                    unsafe { std::env::set_var(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE, previous) };
                }
                None => {
                    // Safety: see above.
                    unsafe { std::env::remove_var(QUADLET_DIRECTORY_ENVIRONMENT_VARIABLE) };
                }
            }
        }
    }

    fn application_id() -> ApplicationId {
        ApplicationId::from(APPLICATION_ID)
    }

    fn application_name() -> ApplicationName {
        ApplicationName::new("orchard").unwrap()
    }

    fn stable_unit() -> String {
        format!("pneuma-orchard-{DEPLOYMENT_ID}")
    }

    fn migrated_connection() -> Connection {
        database::open(Path::new(":memory:")).unwrap()
    }

    // Seeds one deployed application whose active succeeded deployment owns a running
    // runtime record bound to the recorded external container identity.
    fn seed_deployed_runtime(connection: &Connection, desired_state: &str) {
        let digest = format!("sha256:{}", "a".repeat(64));
        connection
            .execute_batch(&format!(
                "INSERT INTO applications (id, name, desired_runtime_state, spec_version, created_at, updated_at)
                 VALUES ('{APPLICATION_ID}', 'orchard', '{desired_state}', 3, '2026-01-01', '2026-01-01');
                 INSERT INTO releases (id, application_id, image_reference, image_repository, image_digest, created_at)
                 VALUES ('22222222222222222222222222222222', '{APPLICATION_ID}', 'registry.example/team/orchard@{digest}', 'registry.example/team/orchard', '{digest}', '2026-01-01');
                 INSERT INTO deployments (id, application_id, release_id, type, status, requested_at, started_at, finished_at)
                 VALUES ('{DEPLOYMENT_ID}', '{APPLICATION_ID}', '22222222222222222222222222222222', 'deploy', 'succeeded', '2026-01-01', '2026-01-01', '2026-01-01');
                 INSERT INTO runtime_instances (id, application_id, deployment_id, external_runtime_id, state, host_address, host_port, container_port, last_observed_state, last_observed_at)
                 VALUES ('{RUNTIME_ID}', '{APPLICATION_ID}', '{DEPLOYMENT_ID}', '{RECORDED_CONTAINER_ID}', 'running', '127.0.0.1', 30000, 8080, 'running', '2026-01-01');
                 UPDATE applications SET active_deployment_id = '{DEPLOYMENT_ID}' WHERE id = '{APPLICATION_ID}';"
            ))
            .unwrap();
    }

    fn persisted_desired_state(connection: &Connection) -> String {
        connection
            .query_row(
                "SELECT desired_runtime_state FROM applications",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn persisted_observed_state(connection: &Connection) -> String {
        connection
            .query_row(
                "SELECT last_observed_state FROM runtime_instances",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn persisted_external_id(connection: &Connection) -> String {
        connection
            .query_row(
                "SELECT external_runtime_id FROM runtime_instances",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn stopping_accepts_a_missing_container_and_keeps_the_runtime_operable() {
        let _scenario = RuntimeScenario::new("stop-missing");
        let mut connection = migrated_connection();
        seed_deployed_runtime(&connection, "running");

        let observation =
            stop_application(&mut connection, &application_id(), &application_name()).unwrap();

        assert_eq!(observation.runtime_id.as_str(), RUNTIME_ID);
        assert_eq!(observation.container_id.as_str(), RECORDED_CONTAINER_ID);
        assert_eq!(
            observation.observed_runtime_state,
            ObservedRuntimeState::Missing
        );
        assert_eq!(observation.observed_endpoint, None);
        assert_eq!(persisted_desired_state(&connection), "stopped");
        assert_eq!(persisted_observed_state(&connection), "missing");

        // The runtime record survives, so status keeps working instead of reporting
        // the application as undeployed.
        let follow_up =
            report_application_status(&mut connection, &application_id(), &application_name())
                .unwrap();
        assert_eq!(
            follow_up.desired_runtime_state,
            DesiredRuntimeState::Stopped
        );
        assert_eq!(
            follow_up.observed_runtime_state,
            ObservedRuntimeState::Missing
        );
    }

    #[test]
    fn starting_recovers_a_recreated_container_through_the_stable_identity() {
        let scenario = RuntimeScenario::new("start-recovery");
        scenario.set_var("PNEUMA_FAKE_SYSTEMCTL_CREATES", RECREATED_CONTAINER_ID);
        scenario.install_unit(&stable_unit());
        let mut connection = migrated_connection();
        seed_deployed_runtime(&connection, "stopped");

        let observation =
            start_application(&mut connection, &application_id(), &application_name()).unwrap();

        assert_eq!(observation.runtime_id.as_str(), RUNTIME_ID);
        assert_eq!(observation.container_id.as_str(), RECREATED_CONTAINER_ID);
        assert_eq!(
            observation.observed_runtime_state,
            ObservedRuntimeState::Running
        );
        assert_eq!(
            observation.observed_endpoint,
            Some("127.0.0.1:31000".parse().unwrap())
        );
        assert_eq!(persisted_desired_state(&connection), "running");
        assert_eq!(persisted_observed_state(&connection), "running");
        // The stable Quadlet identity replaced the stale recorded container id.
        assert_eq!(persisted_external_id(&connection), RECREATED_CONTAINER_ID);
        assert!(
            !scenario
                .podman_invocations()
                .iter()
                .any(|line| line.starts_with("start "))
        );
        assert_eq!(
            scenario.systemctl_invocations(),
            vec![format!("--user start {}.service", stable_unit())]
        );
    }

    #[test]
    fn a_stale_runtime_record_surfaces_as_runtime_changed_instead_of_silent_success() {
        let scenario = RuntimeScenario::new("stale-record");
        scenario.seed_container_present(RECORDED_CONTAINER_ID);
        let mut connection = migrated_connection();
        seed_deployed_runtime(&connection, "running");
        // Simulates a concurrent operator action retiring the runtime between intent
        // persistence and observation recording.
        connection
            .execute_batch(
                "CREATE TRIGGER simulate_concurrent_retirement
                 AFTER UPDATE OF desired_runtime_state ON applications
                 BEGIN
                   UPDATE runtime_instances SET removed_at = '2026-01-02' WHERE application_id = NEW.id;
                 END;",
            )
            .unwrap();

        let error =
            stop_application(&mut connection, &application_id(), &application_name()).unwrap_err();

        assert!(matches!(
            error,
            RuntimeLifecycleError::RuntimeChanged { .. }
        ));
        assert_eq!(persisted_desired_state(&connection), "stopped");
    }

    #[test]
    fn stopping_prefers_the_supervised_unit_over_direct_container_control() {
        let scenario = RuntimeScenario::new("stop-supervised");
        scenario.seed_container_present(RECORDED_CONTAINER_ID);
        scenario.install_unit(&stable_unit());
        let mut connection = migrated_connection();
        seed_deployed_runtime(&connection, "running");

        let observation =
            stop_application(&mut connection, &application_id(), &application_name()).unwrap();

        assert_eq!(
            observation.desired_runtime_state,
            DesiredRuntimeState::Stopped
        );
        assert_eq!(
            observation.observed_runtime_state,
            ObservedRuntimeState::Missing
        );
        assert_eq!(
            scenario.systemctl_invocations(),
            vec![format!("--user stop {}.service", stable_unit())]
        );
        assert!(
            !scenario
                .podman_invocations()
                .iter()
                .any(|line| line.starts_with("stop "))
        );
        assert_eq!(persisted_observed_state(&connection), "missing");
    }

    #[test]
    fn stopping_without_a_unit_controls_the_container_directly() {
        let scenario = RuntimeScenario::new("stop-direct");
        scenario.seed_container_present(RECORDED_CONTAINER_ID);
        let mut connection = migrated_connection();
        seed_deployed_runtime(&connection, "running");

        let observation =
            stop_application(&mut connection, &application_id(), &application_name()).unwrap();

        assert_eq!(
            observation.desired_runtime_state,
            DesiredRuntimeState::Stopped
        );
        assert_eq!(
            observation.observed_runtime_state,
            ObservedRuntimeState::Missing
        );
        assert!(
            scenario
                .podman_invocations()
                .contains(&format!("stop {RECORDED_CONTAINER_ID}"))
        );
        assert!(scenario.systemctl_invocations().is_empty());
        assert_eq!(persisted_observed_state(&connection), "missing");
    }

    #[test]
    fn failed_control_leaves_the_persisted_intent_behind() {
        let scenario = RuntimeScenario::new("failed-control");
        scenario.seed_container_present(RECORDED_CONTAINER_ID);
        scenario.install_unit(&stable_unit());
        scenario.set_var("PNEUMA_FAKE_SYSTEMCTL_EXIT", "1");
        let mut connection = migrated_connection();
        seed_deployed_runtime(&connection, "running");

        let error =
            stop_application(&mut connection, &application_id(), &application_name()).unwrap_err();

        assert!(matches!(
            error,
            RuntimeLifecycleError::Supervision {
                operation: "stopping",
                ..
            }
        ));
        // The intent was recorded before the external effect, and the failed
        // observation was not recorded over the last known state.
        assert_eq!(persisted_desired_state(&connection), "stopped");
        assert_eq!(persisted_observed_state(&connection), "running");
    }
}
