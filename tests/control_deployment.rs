use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pneuma::control::{Command, CommandResult, ControlExecutor, HostConfiguration};
use pneuma::use_cases::ci::{CiCommand, parse_ci_command};
use pneuma::use_cases::deployment::{DeploymentEvent, DeploymentStep};

// Deploy image, branch, rollback, and the restricted CI grammar all cross the
// library boundary without Clap or terminal output.
#[test]
fn deployment_commands_execute_through_the_control_boundary() {
    let scenario = Scenario::new();
    let repository = scenario.create_repository();
    let executor = scenario.executor();

    let imported = executor
        .execute(Command::ImportApplication {
            repository: repository.url(),
            system_name: None,
            manifest_path: None,
        })
        .unwrap();
    assert!(matches!(
        imported,
        CommandResult::ApplicationImported(ref application)
            if application.name.as_str() == "another-site"
    ));

    let artifact = format!("registry.example/team/service@sha256:{}", "a".repeat(64));
    let first = scenario.run_with_health('a', '1', || {
        executor.execute(Command::DeployImage {
            application_name: "another-site".to_owned(),
            image_reference: artifact.clone(),
        })
    });
    let first_deployment_id = deployed(first);

    let mut events = Vec::new();
    let second = scenario.run_with_health('b', '2', || {
        executor.execute_with_events(
            Command::DeployBranch {
                application_name: "another-site".to_owned(),
                branch: "main".to_owned(),
            },
            &mut |event| events.push(event),
        )
    });
    let second_deployment_id = deployed(second);

    let ci_command = parse_ci_command("deploy another-site staging").unwrap();
    let CiCommand::Deploy {
        application,
        branch,
    } = ci_command
    else {
        panic!("CI deploy grammar must produce a deployment command");
    };
    let third = scenario.run_with_health('c', '3', || {
        executor.execute_with_events(
            Command::DeployBranch {
                application_name: application,
                branch,
            },
            &mut |event| events.push(event),
        )
    });
    let third_deployment_id = deployed(third);

    let rollback = scenario.run_with_health('a', '4', || {
        executor.execute_with_events(
            Command::Rollback {
                application_name: "another-site".to_owned(),
            },
            &mut |event| events.push(event),
        )
    });
    let CommandResult::ApplicationRolledBack {
        application_name,
        deployment,
    } = rollback.unwrap()
    else {
        panic!("Rollback must yield ApplicationRolledBack");
    };
    assert_eq!(application_name.as_str(), "another-site");
    assert_ne!(deployment.deployment_id.as_str(), first_deployment_id);
    assert_ne!(deployment.deployment_id.as_str(), second_deployment_id);
    assert_ne!(deployment.deployment_id.as_str(), third_deployment_id);

    let requested: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            DeploymentEvent::DeploymentRequested { application_name } => {
                Some(application_name.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(requested, ["another-site", "another-site", "another-site"]);
    assert!(events.iter().any(|event| matches!(
        event,
        DeploymentEvent::StepStarted {
            step: DeploymentStep::ResolveBranch
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        DeploymentEvent::StepStarted {
            step: DeploymentStep::PullImage
        }
    )));

    let connection = pneuma::adapters::database::open(&scenario.database_path).unwrap();
    let deployment_types: Vec<String> = connection
        .prepare("SELECT type FROM deployments ORDER BY requested_at, rowid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(deployment_types, ["deploy", "deploy", "deploy", "rollback"]);
}

fn deployed(result: Result<CommandResult, pneuma::control::ControlError>) -> String {
    let CommandResult::ApplicationDeployed {
        application_name,
        deployment,
    } = result.unwrap()
    else {
        panic!("deploy commands must yield ApplicationDeployed");
    };
    assert_eq!(application_name.as_str(), "another-site");
    deployment.deployment_id.as_str().to_owned()
}

const FAKE_PODMAN: &str = "#!/bin/sh
case \"$1\" in
  image)
    for argument in \"$@\"; do
      case \"$argument\" in
        *@sha256:*) printf '%s\\n' \"${argument#*@}\"; exit 0 ;;
      esac
    done
    printf 'sha256:%s\\n' \"$PNEUMA_FAKE_DIGEST\"
    ;;
  pull|start) exit 0 ;;
  image) printf 'sha256:%s\\n' \"$PNEUMA_FAKE_DIGEST\" ;;
  inspect)
    if [ \"$2\" = \"--format\" ] && [ \"$3\" = \"{{.Id}}\" ]; then
      printf '%s\\n' \"$PNEUMA_FAKE_CONTAINER_ID\"
    else
      printf 'running\\n'
    fi
    ;;
  port) printf '127.0.0.1:%s\\n' \"$PNEUMA_FAKE_PORT\" ;;
  container) exit 0 ;;
  *) exit 0 ;;
esac
";

const FAKE_SYSTEMCTL: &str = "#!/bin/sh
exit 0
";

static ENVIRONMENT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct Scenario {
    root: PathBuf,
    database_path: PathBuf,
    _environment_guard: MutexGuard<'static, ()>,
    previous_path: Option<std::ffi::OsString>,
    previous_quadlet_dir: Option<std::ffi::OsString>,
    previous_port_range: Option<std::ffi::OsString>,
}

impl Scenario {
    fn new() -> Self {
        let root = temporary_root();
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join("podman"), FAKE_PODMAN);
        write_executable(&bin.join("systemctl"), FAKE_SYSTEMCTL);
        let database_path = root.join("database.sqlite3");
        let _environment_guard = ENVIRONMENT_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let previous_path = env::var_os("PATH");
        let previous_quadlet_dir = env::var_os("PNEUMA_QUADLET_DIR");
        let previous_port_range = env::var_os("PNEUMA_RUNTIME_PORT_RANGE");
        let path = env::join_paths(
            std::iter::once(bin.clone()).chain(
                previous_path
                    .as_ref()
                    .into_iter()
                    .flat_map(|path| env::split_paths(path)),
            ),
        )
        .unwrap();
        unsafe {
            env::set_var("PATH", path);
            env::set_var("PNEUMA_QUADLET_DIR", root.join("quadlets"));
        }
        Self {
            root,
            database_path,
            _environment_guard,
            previous_path,
            previous_quadlet_dir,
            previous_port_range,
        }
    }

    fn executor(&self) -> ControlExecutor {
        ControlExecutor::new(HostConfiguration::new(
            self.database_path.clone(),
            self.root.join("checkouts"),
            self.root.join("caddy"),
            self.root.join("Caddyfile"),
        ))
    }

    fn create_repository(&self) -> GitRepository {
        let path = self.root.join("repository");
        fs::create_dir(&path).unwrap();
        fs::copy(
            fixture_path("another").join("pneuma.toml"),
            path.join("pneuma.toml"),
        )
        .unwrap();
        run_git(&path, &["init", "--quiet", "--initial-branch=main"]);
        run_git(&path, &["add", "pneuma.toml"]);
        run_git(
            &path,
            &[
                "-c",
                "user.name=Pneuma Tests",
                "-c",
                "user.email=pneuma@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ],
        );
        run_git(&path, &["checkout", "--quiet", "-b", "staging"]);
        fs::write(path.join("staging.txt"), "staging\n").unwrap();
        run_git(&path, &["add", "staging.txt"]);
        run_git(
            &path,
            &[
                "-c",
                "user.name=Pneuma Tests",
                "-c",
                "user.email=pneuma@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "staging",
            ],
        );
        run_git(&path, &["checkout", "--quiet", "main"]);
        GitRepository { path }
    }

    fn run_with_health<T, E>(
        &self,
        digest: char,
        container: char,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let container_id = container.to_string().repeat(64);
        unsafe {
            env::set_var("PNEUMA_FAKE_DIGEST", digest.to_string().repeat(64));
            env::set_var("PNEUMA_FAKE_CONTAINER_ID", container_id);
            env::set_var("PNEUMA_FAKE_PORT", port.to_string());
            env::set_var("PNEUMA_RUNTIME_PORT_RANGE", format!("{port}-{port}"));
        }
        let server = thread::spawn(move || respond_once(&listener));
        match operation() {
            Ok(result) => {
                server.join().unwrap();
                Ok(result)
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for Scenario {
    fn drop(&mut self) {
        restore_variable("PATH", self.previous_path.take());
        restore_variable("PNEUMA_QUADLET_DIR", self.previous_quadlet_dir.take());
        restore_variable("PNEUMA_RUNTIME_PORT_RANGE", self.previous_port_range.take());
        fs::remove_dir_all(&self.root).unwrap();
    }
}

struct GitRepository {
    path: PathBuf,
}

impl GitRepository {
    fn url(&self) -> String {
        format!("file://{}", self.path.display())
    }
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn run_git(path: &Path, arguments: &[&str]) {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn respond_once(listener: &TcpListener) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(stream) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "candidate never made a health request"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("failed to accept health request: {error}"),
        }
    };
    let mut request = Vec::new();
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let mut buffer = [0; 1024];
        let read = stream.read(&mut buffer).unwrap();
        assert_ne!(read, 0);
        request.extend_from_slice(&buffer[..read]);
    }
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
        .unwrap();
}

fn restore_variable(name: &str, value: Option<std::ffi::OsString>) {
    unsafe {
        match value {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn temporary_root() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "pneuma-control-deployment-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
