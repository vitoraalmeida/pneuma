use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor, HostConfiguration};
use pneuma::domain::application::DesiredRuntimeState;
use pneuma::domain::runtime::ObservedRuntimeState;
use pneuma::use_cases::application::{ApplicationLookupError, RuntimeLifecycleError};

const APPLICATION_ID: &str = "11111111111111111111111111111111";
const DEPLOYMENT_ID: &str = "33333333333333333333333333333333";
const RUNTIME_ID: &str = "44444444444444444444444444444444";
const RECORDED_CONTAINER_ID: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RECREATED_CONTAINER_ID: &str =
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

// Executes the runtime lifecycle command families through the library boundary
// without Clap, terminal output, or exit codes, receiving only typed results.
#[test]
fn status_reports_the_observed_runtime_through_the_boundary() {
    let scenario = Scenario::new("lifecycle-status");
    scenario.seed_deployed_runtime("running");
    scenario.seed_container_present(RECORDED_CONTAINER_ID);

    let result = scenario
        .executor()
        .execute(Command::ApplicationStatus {
            application_name: "orchard".to_owned(),
        })
        .unwrap();

    let CommandResult::ApplicationStatus {
        application_name,
        observation,
    } = result
    else {
        panic!("ApplicationStatus must yield ApplicationStatus");
    };
    assert_eq!(application_name.as_str(), "orchard");
    assert_eq!(observation.runtime_id.as_str(), RUNTIME_ID);
    assert_eq!(observation.container_id.as_str(), RECORDED_CONTAINER_ID);
    assert_eq!(
        observation.desired_runtime_state,
        DesiredRuntimeState::Running
    );
    assert_eq!(
        observation.observed_runtime_state,
        ObservedRuntimeState::Running
    );
    assert_eq!(
        observation.observed_endpoint,
        Some("127.0.0.1:31000".parse().unwrap())
    );
    assert_eq!(scenario.persisted_observed_state(), "running");
}

#[test]
fn stop_controls_the_container_directly_and_records_the_outcome() {
    let scenario = Scenario::new("lifecycle-stop");
    scenario.seed_deployed_runtime("running");
    scenario.seed_container_present(RECORDED_CONTAINER_ID);

    let result = scenario
        .executor()
        .execute(Command::ApplicationStop {
            application_name: "orchard".to_owned(),
        })
        .unwrap();

    let CommandResult::ApplicationStopped {
        application_name,
        observation,
    } = result
    else {
        panic!("ApplicationStop must yield ApplicationStopped");
    };
    assert_eq!(application_name.as_str(), "orchard");
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
            .contains(&format!("stop {RECORDED_CONTAINER_ID}")),
        "an unsupervised stop must control the container directly"
    );
    assert!(scenario.systemctl_invocations().is_empty());
    assert_eq!(scenario.persisted_desired_state(), "stopped");
    assert_eq!(scenario.persisted_observed_state(), "missing");
}

#[test]
fn start_recovers_a_recreated_container_through_the_supervised_identity() {
    let scenario = Scenario::new("lifecycle-start");
    scenario.seed_deployed_runtime("stopped");
    scenario.install_unit(&stable_unit());
    scenario.set_var("PNEUMA_FAKE_SYSTEMCTL_CREATES", RECREATED_CONTAINER_ID);

    let result = scenario
        .executor()
        .execute(Command::ApplicationStart {
            application_name: "orchard".to_owned(),
        })
        .unwrap();

    let CommandResult::ApplicationStarted {
        application_name,
        observation,
    } = result
    else {
        panic!("ApplicationStart must yield ApplicationStarted");
    };
    assert_eq!(application_name.as_str(), "orchard");
    assert_eq!(
        observation.desired_runtime_state,
        DesiredRuntimeState::Running
    );
    assert_eq!(
        observation.observed_runtime_state,
        ObservedRuntimeState::Running
    );
    assert_eq!(observation.container_id.as_str(), RECREATED_CONTAINER_ID);
    assert_eq!(
        scenario.systemctl_invocations(),
        vec![format!("--user start {}.service", stable_unit())]
    );
    assert_eq!(scenario.persisted_desired_state(), "running");
    assert_eq!(scenario.persisted_observed_state(), "running");
    assert_eq!(scenario.persisted_external_id(), RECREATED_CONTAINER_ID);
}

#[test]
fn lifecycle_commands_for_a_missing_application_are_typed_not_found_errors() {
    let scenario = Scenario::new("lifecycle-missing");
    let executor = scenario.executor();

    for command in [
        Command::ApplicationStatus {
            application_name: "missing".to_owned(),
        },
        Command::ApplicationStop {
            application_name: "missing".to_owned(),
        },
        Command::ApplicationStart {
            application_name: "missing".to_owned(),
        },
    ] {
        let error = executor.execute(command).unwrap_err();
        assert!(
            matches!(
                error,
                ControlError::ApplicationLookup {
                    source: ApplicationLookupError::NotFound { .. }
                }
            ),
            "a missing application must be a typed not-found lookup error"
        );
    }
}

#[test]
fn lifecycle_commands_for_an_undeployed_application_fail_with_typed_not_deployed_errors() {
    let scenario = Scenario::new("lifecycle-undeployed");
    scenario.seed_undeployed_application();
    let executor = scenario.executor();

    let error = executor
        .execute(Command::ApplicationStatus {
            application_name: "orchard".to_owned(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        ControlError::RuntimeStatus {
            source: RuntimeLifecycleError::NotDeployed { .. }
        }
    ));

    let error = executor
        .execute(Command::ApplicationStart {
            application_name: "orchard".to_owned(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        ControlError::RuntimeStart {
            source: RuntimeLifecycleError::NotDeployed { .. }
        }
    ));

    let error = executor
        .execute(Command::ApplicationStop {
            application_name: "orchard".to_owned(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        ControlError::RuntimeStop {
            source: RuntimeLifecycleError::NotDeployed { .. }
        }
    ));
}

fn stable_unit() -> String {
    format!("pneuma-orchard-{DEPLOYMENT_ID}")
}

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

// Environment overrides are process-global, so all scenarios in this binary
// serialize their setup, execution, and teardown behind one mutex.
static ENVIRONMENT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn environment_lock() -> &'static Mutex<()> {
    ENVIRONMENT_LOCK.get_or_init(|| Mutex::new(()))
}

// Scopes the fake PATH, the Quadlet directory, and all behavior variables of
// one scenario; logs stay readable for asserting which tool controlled the
// runtime.
struct Scenario {
    root: PathBuf,
    _environment_guard: MutexGuard<'static, ()>,
    previous_path: Option<std::ffi::OsString>,
    previous_quadlet_directory: Option<std::ffi::OsString>,
}

impl Scenario {
    fn new(name: &str) -> Self {
        let root = temporary_root(name);
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for (file_name, contents) in [("podman", FAKE_PODMAN), ("systemctl", FAKE_SYSTEMCTL)] {
            let script = bin.join(file_name);
            fs::write(&script, contents).unwrap();
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).unwrap();
        }
        let quadlet_directory = root.join("quadlets");
        fs::create_dir_all(&quadlet_directory).unwrap();
        fs::write(root.join("database.sqlite3"), []).unwrap();
        fs::create_dir_all(root.join("checkouts")).unwrap();

        let _environment_guard = environment_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for variable in [
            "PNEUMA_FAKE_PODMAN_STATUS",
            "PNEUMA_FAKE_PODMAN_PORT",
            "PNEUMA_FAKE_PODMAN_INSPECT_EXIT",
            "PNEUMA_FAKE_PODMAN_EXIT",
            "PNEUMA_FAKE_SYSTEMCTL_EXIT",
            "PNEUMA_FAKE_SYSTEMCTL_CREATES",
        ] {
            // Safety: see above.
            unsafe { env::remove_var(variable) };
        }
        let previous_path = env::var_os("PATH");
        // Safety: environment reads and writes happen while holding
        // ENVIRONMENT_LOCK, which the returned guard keeps alive.
        unsafe { env::set_var("PATH", &bin) };
        let previous_quadlet_directory = env::var_os("PNEUMA_QUADLET_DIR");
        unsafe { env::set_var("PNEUMA_QUADLET_DIR", &quadlet_directory) };
        unsafe {
            env::set_var("PNEUMA_FAKE_PODMAN_LOG", root.join("podman.log"));
            env::set_var("PNEUMA_FAKE_SYSTEMCTL_LOG", root.join("systemctl.log"));
            env::set_var("PNEUMA_FAKE_PODMAN_STATE", root.join("container.state"));
        }
        Self {
            root,
            _environment_guard,
            previous_path,
            previous_quadlet_directory,
        }
    }

    fn executor(&self) -> ControlExecutor {
        ControlExecutor::new(HostConfiguration::new(
            self.root.join("database.sqlite3"),
            self.root.join("checkouts"),
        ))
    }

    fn set_var(&self, name: &str, value: &str) {
        // Safety: see Scenario::new.
        unsafe { env::set_var(name, value) };
    }

    // Emulates the container recorded by the runtime being alive before the
    // scenario's command runs.
    fn seed_container_present(&self, id: &str) {
        fs::write(self.root.join("container.state"), format!("{id}\n")).unwrap();
    }

    // Materializes the stable Quadlet unit so supervision is preferred.
    fn install_unit(&self, unit: &str) {
        fs::write(
            self.root.join("quadlets").join(format!("{unit}.container")),
            "",
        )
        .unwrap();
    }

    // Seeds one deployed application whose active succeeded deployment owns a
    // running runtime record bound to the recorded external container identity.
    fn seed_deployed_runtime(&self, desired_state: &str) {
        let digest = format!("sha256:{}", "a".repeat(64));
        self.connection()
            .execute_batch(&format!(
                "INSERT INTO systems (id, name) VALUES ('22222222222222222222222222222222', 'team');
                 INSERT INTO applications (id, system_id, name, repository_url, manifest_path, image_repository, container_port, health_check_path, health_check_expected_status, desired_runtime_state)
                 VALUES ('{APPLICATION_ID}', '22222222222222222222222222222222', 'orchard', 'https://example.test/app.git', 'pneuma.toml', 'registry.example/team/orchard', 8080, '/healthz', 200, '{desired_state}');
                 INSERT INTO releases (id, application_id, image_reference, created_at)
                 VALUES ('55555555555555555555555555555555', '{APPLICATION_ID}', 'registry.example/team/orchard@{digest}', '2026-01-01');
                 INSERT INTO deployments (id, application_id, release_id, type, status, requested_at, started_at, finished_at)
                 VALUES ('{DEPLOYMENT_ID}', '{APPLICATION_ID}', '55555555555555555555555555555555', 'deploy', 'succeeded', '2026-01-01', '2026-01-01', '2026-01-01');
                 INSERT INTO runtime_instances (id, application_id, deployment_id, external_runtime_id, state, host_port, container_port, last_observed_state, last_observed_at)
                 VALUES ('{RUNTIME_ID}', '{APPLICATION_ID}', '{DEPLOYMENT_ID}', '{RECORDED_CONTAINER_ID}', 'running', 30000, 8080, 'running', '2026-01-01');
                 UPDATE applications SET active_deployment_id = '{DEPLOYMENT_ID}' WHERE id = '{APPLICATION_ID}';"
            ))
            .unwrap();
    }

    // Seeds one registered application without any deployment so lifecycle
    // commands hit the not-deployed rule.
    fn seed_undeployed_application(&self) {
        self.connection()
            .execute_batch(&format!(
                "INSERT INTO systems (id, name) VALUES ('22222222222222222222222222222222', 'team');
                 INSERT INTO applications (id, system_id, name, repository_url, manifest_path, image_repository, container_port, health_check_path, health_check_expected_status, desired_runtime_state)
                 VALUES ('{APPLICATION_ID}', '22222222222222222222222222222222', 'orchard', 'https://example.test/app.git', 'pneuma.toml', 'registry.example/team/orchard', 8080, '/healthz', 200, 'running');"
            ))
            .unwrap();
    }

    fn connection(&self) -> rusqlite::Connection {
        pneuma::adapters::database::open(&self.root.join("database.sqlite3")).unwrap()
    }

    fn podman_invocations(&self) -> Vec<String> {
        self.invocations("podman.log")
    }

    fn systemctl_invocations(&self) -> Vec<String> {
        self.invocations("systemctl.log")
    }

    fn invocations(&self, log: &str) -> Vec<String> {
        fs::read_to_string(self.root.join(log))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn persisted_desired_state(&self) -> String {
        self.connection()
            .query_row(
                "SELECT desired_runtime_state FROM applications",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn persisted_observed_state(&self) -> String {
        self.connection()
            .query_row(
                "SELECT last_observed_state FROM runtime_instances",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn persisted_external_id(&self) -> String {
        self.connection()
            .query_row(
                "SELECT external_runtime_id FROM runtime_instances",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }
}

impl Drop for Scenario {
    fn drop(&mut self) {
        match self.previous_path.take() {
            Some(previous) => {
                // Safety: see Scenario::new.
                unsafe { env::set_var("PATH", previous) };
            }
            None => {
                // Safety: see Scenario::new.
                unsafe { env::remove_var("PATH") };
            }
        }
        match self.previous_quadlet_directory.take() {
            Some(previous) => {
                // Safety: see Scenario::new.
                unsafe { env::set_var("PNEUMA_QUADLET_DIR", previous) };
            }
            None => {
                // Safety: see Scenario::new.
                unsafe { env::remove_var("PNEUMA_QUADLET_DIR") };
            }
        }
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn temporary_root(name: &str) -> PathBuf {
    let root = env::temp_dir().join(format!(
        "pneuma-control-lifecycle-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
