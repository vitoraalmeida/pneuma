use std::cell::RefCell;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::database;
use pneuma::domain::deployment::DeploymentStatus;
use pneuma::use_cases::application::import_application;
use pneuma::use_cases::deployment::{
    DeployBranchError, DeploymentEvent, DeploymentStep, deploy_branch, deploy_branch_with_events,
};

#[test]
fn deploys_a_branch_and_persists_source_revision() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = GitRepository::new();
    let staging_commit = repository.commit_on_new_branch("staging", "staging contents");
    let application = import_application(
        &mut connection,
        &repository.path,
        None,
        &repository.url(),
        None,
    )
    .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let environment = FakePodman::new(port);
    let server = thread::spawn(move || respond_once(&listener));

    let deployed =
        environment.run(|| deploy_branch(&mut connection, &application.id, Some("staging"), None));
    server.join().unwrap();

    let deployed = deployed.unwrap();
    assert!(deployed.source_revision.is_some());
    assert_eq!(
        deployed
            .source_revision
            .as_ref()
            .map(|commit| commit.as_str().to_owned()),
        Some(staging_commit.as_str().to_owned())
    );
    let (source_revision, image_reference): (Option<String>, String) = connection
        .query_row(
            "SELECT d.source_revision, r.image_reference
             FROM deployments d
             JOIN releases r ON r.id = d.release_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(source_revision.as_deref(), Some(staging_commit.as_str()));
    let unit = fs::read_to_string(environment.root.join("quadlets").join(format!(
        "pneuma-another-site-{}.container",
        deployed.deployment_id
    )))
    .unwrap();
    assert!(unit.contains(&format!(
        "Label=io.pneuma.image-digest=sha256:{}",
        "a".repeat(64)
    )));
    assert!(!unit.contains(&format!("io.pneuma.revision={staging_commit}")));
    assert!(environment.log().contains(&format!(
        "pull --quiet registry.example/team/service:{staging_commit}"
    )));
    assert_eq!(deployed.artifact.reference(), image_reference);
}

#[test]
fn uses_the_default_branch_when_branch_is_omitted() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = GitRepository::new();
    let main_commit = repository.head_commit();
    let application = import_application(
        &mut connection,
        &repository.path,
        None,
        &repository.url(),
        None,
    )
    .unwrap();
    connection
        .execute(
            "UPDATE applications SET default_branch = 'main' WHERE id = ?1",
            [application.id.as_str()],
        )
        .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let environment = FakePodman::new(port);
    let server = thread::spawn(move || respond_once(&listener));

    let deployed = environment.run(|| deploy_branch(&mut connection, &application.id, None, None));
    server.join().unwrap();

    let deployed = deployed.unwrap();
    assert_eq!(
        deployed
            .source_revision
            .as_ref()
            .map(|commit| commit.as_str().to_owned()),
        Some(main_commit.as_str().to_owned())
    );
}

#[test]
fn fails_for_a_missing_branch() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = GitRepository::new();
    let application = import_application(
        &mut connection,
        &repository.path,
        None,
        &repository.url(),
        None,
    )
    .unwrap();

    let error = deploy_branch(&mut connection, &application.id, Some("missing"), None).unwrap_err();

    assert!(matches!(
        error,
        DeployBranchError::ResolveBranch {
            source: pneuma::adapters::git_source::ResolveBranchError::BranchNotFound { .. }
        }
    ));
    let release_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM releases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(release_count, 0);
}

#[test]
fn deploys_a_branch_with_the_same_semantic_event_order_and_result() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = GitRepository::new();
    let staging_commit = repository.commit_on_new_branch("staging", "staging contents");
    let application = import_application(
        &mut connection,
        &repository.path,
        None,
        &repository.url(),
        None,
    )
    .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let environment = FakePodman::new(port);
    let server = thread::spawn(move || respond_once(&listener));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let mut report = move |event: DeploymentEvent| sink.borrow_mut().push(event);

    let deployed = environment.run(|| {
        deploy_branch_with_events(
            &mut connection,
            &application.id,
            Some("staging"),
            None,
            &mut report,
        )
    });
    server.join().unwrap();

    // The reported run must persist the same outcome as the silent branch deployment.
    let deployed = deployed.unwrap();
    assert_eq!(
        deployed
            .source_revision
            .as_ref()
            .map(|commit| commit.as_str().to_owned()),
        Some(staging_commit.as_str().to_owned())
    );
    let (status, source_revision): (String, Option<String>) = connection
        .query_row(
            "SELECT d.status, d.source_revision FROM deployments d",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "succeeded");
    assert_eq!(source_revision.as_deref(), Some(staging_commit.as_str()));

    let observed: Vec<EventShape> = events.borrow().iter().map(event_shape).collect();
    assert_eq!(observed, internal_deployment_progress_sequence());
}

#[test]
fn fails_identically_while_reporting_nothing_when_the_branch_is_missing() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = GitRepository::new();
    let application = import_application(
        &mut connection,
        &repository.path,
        None,
        &repository.url(),
        None,
    )
    .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let mut report = move |event: DeploymentEvent| sink.borrow_mut().push(event);

    let error = deploy_branch_with_events(
        &mut connection,
        &application.id,
        Some("missing"),
        None,
        &mut report,
    )
    .unwrap_err();

    // Source resolution is the first observable operation and fails before later effects.
    assert!(matches!(
        error,
        DeployBranchError::ResolveBranch {
            source: pneuma::adapters::git_source::ResolveBranchError::BranchNotFound { .. }
        }
    ));
    assert!(matches!(
        events.borrow().as_slice(),
        [DeploymentEvent::StepStarted {
            step: DeploymentStep::ResolveBranch
        }]
    ));
}

#[test]
fn fails_for_an_unreachable_registry() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let repository = GitRepository::new();
    let application = import_application(
        &mut connection,
        &repository.path,
        None,
        &repository.url(),
        None,
    )
    .unwrap();
    let environment = FakePodman::new(30000);

    let error = environment
        .run_with_pull_failure(|| {
            deploy_branch(&mut connection, &application.id, Some("main"), None)
        })
        .unwrap_err();

    assert!(matches!(
        error,
        DeployBranchError::ResolveImageDigest {
            source: pneuma::adapters::oci_image::ResolveImageDigestError::Pull { .. }
        }
    ));
    let release_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM releases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(release_count, 0);
}

struct FakePodman {
    root: PathBuf,
    bin: PathBuf,
    log_path: PathBuf,
    port: String,
}

impl FakePodman {
    fn new(port: u16) -> Self {
        let root = env::temp_dir().join(format!(
            "pneuma-deploy-branch-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let log_path = root.join("podman.log");
        let podman = bin.join("podman");
        fs::write(
            &podman,
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_PODMAN_LOG\"\ncase \"$1\" in\n  pull) if [ -f \"${PNEUMA_FAKE_PODMAN_PULL_FAILURE:-}\" ]; then printf 'pull failed\\n' >&2; exit 1; fi ;;\n  start|container) ;;\n  image) printf '%s\\n' \"${PNEUMA_FAKE_DIGEST}\" ;;\n  inspect) if [ \"$2\" = \"--format\" ] && [ \"$3\" = \"{{.Id}}\" ]; then printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'; else printf 'running\\n'; fi ;;\n  port) printf '127.0.0.1:%s\\n' \"$PNEUMA_FAKE_PORT\" ;;\n  *) exit 1 ;;\nesac\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&podman).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&podman, permissions).unwrap();
        let systemctl = bin.join("systemctl");
        fs::write(&systemctl, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&systemctl).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&systemctl, permissions).unwrap();
        Self {
            root,
            bin,
            log_path,
            port: port.to_string(),
        }
    }

    fn run<T>(&self, operation: impl FnOnce() -> T) -> T {
        self.run_internal(operation, None)
    }

    fn run_internal<T>(&self, operation: impl FnOnce() -> T, failure: Option<&Path>) -> T {
        let _environment_lock = environment_lock().lock().unwrap();
        let digest = format!("sha256:{}", "a".repeat(64));
        let path = env::join_paths(
            std::iter::once(self.bin.clone())
                .chain(env::split_paths(&env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let previous_path = env::var_os("PATH");
        unsafe { env::set_var("PATH", path) };
        unsafe { env::set_var("PNEUMA_FAKE_PODMAN_LOG", &self.log_path) };
        unsafe { env::set_var("PNEUMA_FAKE_DIGEST", digest) };
        unsafe { env::set_var("PNEUMA_FAKE_PORT", &self.port) };
        unsafe {
            env::set_var(
                "PNEUMA_RUNTIME_PORT_RANGE",
                format!("{}-{}", self.port, self.port),
            )
        };
        unsafe { env::set_var("PNEUMA_QUADLET_DIR", self.root.join("quadlets")) };
        if let Some(failure) = failure {
            unsafe { env::set_var("PNEUMA_FAKE_PODMAN_PULL_FAILURE", failure) };
        } else {
            unsafe { env::remove_var("PNEUMA_FAKE_PODMAN_PULL_FAILURE") };
        }
        let result = operation();
        if let Some(path) = previous_path {
            unsafe { env::set_var("PATH", path) };
        }
        result
    }

    fn run_with_pull_failure<T>(&self, operation: impl FnOnce() -> T) -> T {
        let failure_path = self.root.join("pull-failure");
        fs::write(&failure_path, "fail").unwrap();
        self.run_internal(operation, Some(&failure_path))
    }

    fn log(&self) -> String {
        fs::read_to_string(&self.log_path).unwrap()
    }
}

impl Drop for FakePodman {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct GitRepository {
    temporary_root: PathBuf,
    path: PathBuf,
}

impl GitRepository {
    fn new() -> Self {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temporary_root = env::temp_dir().join(format!(
            "pneuma-deploy-branch-repo-{}-{unique_suffix}",
            std::process::id()
        ));
        fs::create_dir(&temporary_root).unwrap();
        let path = temporary_root.join("repository");

        let output = Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .arg(&path)
            .output()
            .unwrap();
        assert_git_succeeded(&output);

        fs::copy(
            fixture_path("another").join("pneuma.toml"),
            path.join("pneuma.toml"),
        )
        .unwrap();
        let repository = Self {
            temporary_root,
            path,
        };
        repository.git(&["add", "pneuma.toml"]);
        repository.git(&[
            "-c",
            "user.name=Pneuma Tests",
            "-c",
            "user.email=pneuma@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "initial commit",
        ]);
        repository
    }

    fn git(&self, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(arguments)
            .output()
            .unwrap();
        assert_git_succeeded(&output);
        String::from_utf8(output.stdout).unwrap()
    }

    fn commit_on_new_branch(&self, branch: &str, contents: &str) -> String {
        self.git(&["checkout", "--quiet", "-b", branch]);
        fs::write(self.path.join("site.txt"), contents).unwrap();
        self.git(&["add", "site.txt"]);
        let message = format!("{branch} commit");
        self.git(&[
            "-c",
            "user.name=Pneuma Tests",
            "-c",
            "user.email=pneuma@example.invalid",
            "commit",
            "--quiet",
            "-m",
            &message,
        ]);
        self.head_commit()
    }

    fn head_commit(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn url(&self) -> String {
        format!("file://{}", self.path.display())
    }
}

impl Drop for GitRepository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temporary_root);
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

// Semantic event shape: operation boundaries and persisted state transitions.
#[derive(Debug, PartialEq, Eq)]
enum EventShape {
    Started(DeploymentStep),
    Completed(DeploymentStep),
    StateChanged(DeploymentStatus),
    FailurePersisted,
}

fn event_shape(event: &DeploymentEvent) -> EventShape {
    match event {
        DeploymentEvent::StepStarted { step } => EventShape::Started(*step),
        DeploymentEvent::StepCompleted { step } => EventShape::Completed(*step),
        DeploymentEvent::StateChanged { status, .. } => EventShape::StateChanged(*status),
        DeploymentEvent::FailurePersisted { .. } => EventShape::FailurePersisted,
        DeploymentEvent::RetirementWarning { .. } => {
            panic!("a first deployment has no retirement warning")
        }
    }
}

// The operation order every branch deployment reports, including source resolution.
fn internal_deployment_progress_sequence() -> Vec<EventShape> {
    vec![
        EventShape::Started(DeploymentStep::ResolveBranch),
        EventShape::Completed(DeploymentStep::ResolveBranch),
        EventShape::Started(DeploymentStep::ResolveImageDigest),
        EventShape::Completed(DeploymentStep::ResolveImageDigest),
        EventShape::Started(DeploymentStep::PullImage),
        EventShape::Completed(DeploymentStep::PullImage),
        EventShape::Started(DeploymentStep::LoadSpecification),
        EventShape::Completed(DeploymentStep::LoadSpecification),
        EventShape::Started(DeploymentStep::CreateDeployment),
        EventShape::Completed(DeploymentStep::CreateDeployment),
        EventShape::StateChanged(DeploymentStatus::Pending),
        EventShape::StateChanged(DeploymentStatus::Starting),
        EventShape::Started(DeploymentStep::ReservePort),
        EventShape::Completed(DeploymentStep::ReservePort),
        EventShape::Started(DeploymentStep::CreateUnit),
        EventShape::Completed(DeploymentStep::CreateUnit),
        EventShape::Started(DeploymentStep::ReloadSystemd),
        EventShape::Completed(DeploymentStep::ReloadSystemd),
        EventShape::Started(DeploymentStep::StartContainer),
        EventShape::Completed(DeploymentStep::StartContainer),
        EventShape::Started(DeploymentStep::ResolveContainer),
        EventShape::Completed(DeploymentStep::ResolveContainer),
        EventShape::Started(DeploymentStep::ObserveContainer),
        EventShape::Completed(DeploymentStep::ObserveContainer),
        EventShape::Started(DeploymentStep::RegisterCandidate),
        EventShape::Completed(DeploymentStep::RegisterCandidate),
        EventShape::StateChanged(DeploymentStatus::Verifying),
        EventShape::Started(DeploymentStep::InternalHealthCheck),
        EventShape::Completed(DeploymentStep::InternalHealthCheck),
        EventShape::Started(DeploymentStep::PromoteCandidate),
        EventShape::Completed(DeploymentStep::PromoteCandidate),
        EventShape::StateChanged(DeploymentStatus::Succeeded),
    ]
}

fn respond_once(listener: &TcpListener) {
    let (mut stream, _) = listener.accept().unwrap();
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

fn environment_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn assert_git_succeeded(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
