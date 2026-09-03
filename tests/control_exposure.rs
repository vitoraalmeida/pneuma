use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor, HostConfiguration};
use pneuma::domain::exposure::Visibility;
use pneuma::use_cases::application::ApplicationLookupError;
use pneuma::use_cases::exposure::ExposureChangeError;
use pneuma::use_cases::reconciliation::ReconciliationReadError;

const APPLICATION_ID: &str = "11111111111111111111111111111111";
const DEPLOYMENT_ID: &str = "33333333333333333333333333333333";
const RECORDED_CONTAINER_ID: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// Executes the exposure and reconciliation command families through the library
// boundary without Clap, terminal output, or exit codes, receiving only typed
// results.
#[test]
fn visibility_set_for_a_missing_application_is_a_typed_not_found_error() {
    let scenario = Scenario::new("exposure-missing");

    let error = scenario
        .executor()
        .execute(Command::VisibilitySet {
            application_name: "missing".to_owned(),
            visibility: Visibility::Public,
        })
        .unwrap_err();

    assert!(matches!(
        error,
        ControlError::ApplicationLookup {
            source: ApplicationLookupError::NotFound { .. }
        }
    ));
}

#[test]
fn setting_public_visibility_without_a_domain_is_a_typed_domain_required_error() {
    let scenario = Scenario::new("exposure-domain-required");
    scenario.seed_application();
    scenario.seed_internal_exposure_without_domain();

    let error = scenario
        .executor()
        .execute(Command::VisibilitySet {
            application_name: "orchard".to_owned(),
            visibility: Visibility::Public,
        })
        .unwrap_err();

    assert!(matches!(
        error,
        ControlError::VisibilitySet {
            source: ExposureChangeError::DomainRequired { .. }
        }
    ));
    assert_eq!(scenario.persisted_visibility(), "internal");
}

#[test]
fn making_an_application_public_materializes_the_caddy_route_through_the_boundary() {
    let scenario = Scenario::new("exposure-public");
    scenario.write_caddyfile();
    scenario.seed_deployed_runtime();
    scenario.seed_internal_exposure_with_domain();

    let result = scenario
        .executor()
        .execute(Command::VisibilitySet {
            application_name: "orchard".to_owned(),
            visibility: Visibility::Public,
        })
        .unwrap();

    let CommandResult::ExposureChanged {
        application_name,
        change,
    } = result
    else {
        panic!("VisibilitySet must yield ExposureChanged");
    };
    assert_eq!(application_name.as_str(), "orchard");
    assert_eq!(change.visibility, Visibility::Public);
    assert_eq!(
        change.domain.as_ref().map(|domain| domain.as_str()),
        Some("example.com")
    );
    assert_eq!(
        scenario.active_fragment(),
        "example.com {\n    reverse_proxy 127.0.0.1:31000\n}\n"
    );
    assert!(
        scenario
            .caddy_invocations()
            .iter()
            .all(|command| command.contains("--adapter caddyfile")),
        "caddy must validate and reload the configured Caddyfile: {:?}",
        scenario.caddy_invocations()
    );
    assert!(scenario.curl_invoked());
    assert_eq!(scenario.persisted_visibility(), "public");
    assert_eq!(scenario.persisted_materialization_state(), "active");
}

#[test]
fn reconcile_for_a_missing_application_is_a_typed_not_found_error() {
    let scenario = Scenario::new("reconcile-missing");

    let error = scenario
        .executor()
        .execute(Command::Reconcile {
            application_name: "missing".to_owned(),
        })
        .unwrap_err();

    assert!(matches!(
        error,
        ControlError::Reconcile {
            source: ReconciliationReadError::ApplicationNotFound { .. }
        }
    ));
}

#[test]
fn reconcile_for_an_undeployed_application_reports_not_converged() {
    let scenario = Scenario::new("reconcile-undeployed");
    scenario.seed_application();

    let error = scenario
        .executor()
        .execute(Command::Reconcile {
            application_name: "orchard".to_owned(),
        })
        .unwrap_err();

    match error {
        ControlError::Reconcile {
            source: ReconciliationReadError::NotConverged { reason },
        } => assert_eq!(reason, "application has no active runtime"),
        other => panic!("expected a not-converged reconciliation error, got {other:?}"),
    }
}

// Fake `podman` reporting the one container recorded by the state file as
// running on a fixed loopback port. Only shell builtins are used because the
// scoped PATH exposes no coreutils.
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

// Fake `caddy` accepting every validation and reload so the exposure flow
// reaches its confirmed state.
const FAKE_CADDY: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_CADDY_LOG\"
exit 0
";

// Fake `curl` reporting a successful external health check.
const FAKE_CURL: &str = "#!/bin/sh
printf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_CURL_LOG\"
printf '200'
";

// Environment overrides are process-global, so all scenarios in this binary
// serialize their setup, execution, and teardown behind one mutex.
static ENVIRONMENT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn environment_lock() -> &'static Mutex<()> {
    ENVIRONMENT_LOCK.get_or_init(|| Mutex::new(()))
}

// Scopes the fake PATH and the Caddy behavior variables of one scenario.
struct Scenario {
    root: PathBuf,
    _environment_guard: MutexGuard<'static, ()>,
    previous_path: Option<std::ffi::OsString>,
}

impl Scenario {
    fn new(name: &str) -> Self {
        let root = temporary_root(name);
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for (file_name, contents) in [
            ("podman", FAKE_PODMAN),
            ("caddy", FAKE_CADDY),
            ("curl", FAKE_CURL),
        ] {
            let script = bin.join(file_name);
            fs::write(&script, contents).unwrap();
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).unwrap();
        }
        fs::write(root.join("database.sqlite3"), []).unwrap();
        fs::create_dir_all(root.join("checkouts")).unwrap();
        fs::create_dir_all(root.join("caddy")).unwrap();

        let _environment_guard = environment_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for variable in [
            "PNEUMA_FAKE_PODMAN_STATUS",
            "PNEUMA_FAKE_PODMAN_PORT",
            "PNEUMA_FAKE_PODMAN_INSPECT_EXIT",
            "PNEUMA_FAKE_PODMAN_EXIT",
        ] {
            // Safety: environment reads and writes happen while holding
            // ENVIRONMENT_LOCK, which the returned guard keeps alive.
            unsafe { env::remove_var(variable) };
        }
        let previous_path = env::var_os("PATH");
        // Safety: see above.
        unsafe { env::set_var("PATH", &bin) };
        unsafe {
            env::set_var("PNEUMA_FAKE_PODMAN_LOG", root.join("podman.log"));
            env::set_var("PNEUMA_FAKE_PODMAN_STATE", root.join("container.state"));
            env::set_var("PNEUMA_FAKE_CADDY_LOG", root.join("caddy.log"));
            env::set_var("PNEUMA_FAKE_CURL_LOG", root.join("curl.log"));
        }
        Self {
            root,
            _environment_guard,
            previous_path,
        }
    }

    fn executor(&self) -> ControlExecutor {
        ControlExecutor::new(HostConfiguration::new(
            self.root.join("database.sqlite3"),
            self.root.join("checkouts"),
            self.root.join("caddy"),
            self.root.join("Caddyfile"),
        ))
    }

    fn write_caddyfile(&self) {
        fs::write(
            self.root.join("Caddyfile"),
            "import /etc/caddy/applications/*\n",
        )
        .unwrap();
    }

    // Seeds one registered application.
    fn seed_application(&self) {
        self.connection()
            .execute_batch(&format!(
                "INSERT INTO systems (id, name) VALUES ('22222222222222222222222222222222', 'team');
                 INSERT INTO applications (id, system_id, name, repository_url, manifest_path, image_repository, container_port, health_check_path, health_check_expected_status, desired_runtime_state)
                 VALUES ('{APPLICATION_ID}', '22222222222222222222222222222222', 'orchard', 'https://example.test/app.git', 'pneuma.toml', 'registry.example/team/orchard', 8080, '/healthz', 200, 'running');"
            ))
            .unwrap();
    }

    // Seeds one deployed application with an active succeeded runtime observed
    // running on the fixed loopback endpoint.
    fn seed_deployed_runtime(&self) {
        let digest = format!("sha256:{}", "a".repeat(64));
        self.seed_application();
        self.connection()
            .execute_batch(&format!(
                "INSERT INTO releases (id, application_id, image_reference, created_at)
                 VALUES ('55555555555555555555555555555555', '{APPLICATION_ID}', 'registry.example/team/orchard@{digest}', '2026-01-01');
                 INSERT INTO deployments (id, application_id, release_id, type, status, requested_at, started_at, finished_at)
                 VALUES ('{DEPLOYMENT_ID}', '{APPLICATION_ID}', '55555555555555555555555555555555', 'deploy', 'succeeded', '2026-01-01', '2026-01-01', '2026-01-01');
                 INSERT INTO runtime_instances (id, application_id, deployment_id, external_runtime_id, state, host_port, container_port, last_observed_state, last_observed_at)
                 VALUES ('66666666666666666666666666666666', '{APPLICATION_ID}', '{DEPLOYMENT_ID}', '{RECORDED_CONTAINER_ID}', 'running', 30000, 8080, 'running', '2026-01-01');
                 UPDATE applications SET active_deployment_id = '{DEPLOYMENT_ID}' WHERE id = '{APPLICATION_ID}';"
            ))
            .unwrap();
        fs::write(
            self.root.join("container.state"),
            format!("{RECORDED_CONTAINER_ID}\n"),
        )
        .unwrap();
    }

    fn seed_internal_exposure_without_domain(&self) {
        self.connection()
            .execute_batch(&format!(
                "INSERT INTO exposures (application_id, desired_visibility, domain, materialization_state)
                 VALUES ('{APPLICATION_ID}', 'internal', NULL, 'not_materialized');"
            ))
            .unwrap();
    }

    fn seed_internal_exposure_with_domain(&self) {
        self.connection()
            .execute_batch(&format!(
                "INSERT INTO exposures (application_id, desired_visibility, domain, materialization_state)
                 VALUES ('{APPLICATION_ID}', 'internal', 'example.com', 'not_materialized');"
            ))
            .unwrap();
    }

    fn connection(&self) -> rusqlite::Connection {
        pneuma::adapters::database::open(&self.root.join("database.sqlite3")).unwrap()
    }

    fn caddy_invocations(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("caddy.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn curl_invoked(&self) -> bool {
        fs::read_to_string(self.root.join("curl.log"))
            .unwrap_or_default()
            .contains("https://example.com/healthz")
    }

    fn fragment_path(&self) -> PathBuf {
        self.root
            .join("caddy")
            .join(format!("{APPLICATION_ID}.caddy"))
    }

    fn active_fragment(&self) -> String {
        fs::read_to_string(self.fragment_path()).unwrap()
    }

    fn persisted_visibility(&self) -> String {
        self.connection()
            .query_row(
                "SELECT desired_visibility FROM exposures WHERE application_id = ?1",
                [APPLICATION_ID],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| panic!("the exposure row must exist"))
    }

    fn persisted_materialization_state(&self) -> String {
        self.connection()
            .query_row(
                "SELECT materialization_state FROM exposures WHERE application_id = ?1",
                [APPLICATION_ID],
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
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn temporary_root(name: &str) -> PathBuf {
    let root = env::temp_dir().join(format!(
        "pneuma-control-exposure-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
