use std::cell::RefCell;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::database;
use pneuma::domain::deployment::DeploymentStatus;
use pneuma::domain::release::OciArtifact;
use pneuma::use_cases::application::import_application;
use pneuma::use_cases::deployment::{
    DeployOciError, DeploymentProgress, DeploymentStep, deploy_oci, deploy_oci_with_progress,
};

#[test]
fn deploys_a_verified_oci_image_and_persists_its_exact_reference() {
    let database_path = temporary_database_path();
    let mut connection = database::open(&database_path).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("another"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/team/service@{digest}");
    let artifact = OciArtifact::parse(&reference).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let environment = FakePodman::new(port);
    let server = thread::spawn(move || respond_once(&listener));

    let deployed =
        environment.run(|| deploy_oci(&mut connection, &application.id, &artifact, None, None));
    server.join().unwrap();

    let release = connection
        .query_row(
            "SELECT r.id, r.image_reference, d.source_revision
             FROM releases r
             JOIN deployments d ON d.release_id = r.id",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .unwrap();
    let deployed = deployed.unwrap();

    // The canonical reference is the one persisted artifact identity; the
    // repository and digest are derived from it by parsing.
    assert_eq!(release.1, reference);
    assert_eq!(release.1, format!("registry.example/team/service@{digest}"));
    assert_eq!(release.2, None);
    assert_eq!(deployed.artifact.reference(), release.1);
    assert_eq!(
        deployed
            .source_revision
            .as_ref()
            .map(|commit| commit.as_str().to_owned()),
        release.2.clone()
    );
    assert!(environment.log().contains(&reference));
    assert!(
        environment
            .log()
            .contains("pull registry.example/team/service@sha256:")
    );
    drop(connection);
    fs::remove_file(database_path).unwrap();
}

#[test]
fn rejects_an_unpinned_oci_reference_at_the_validation_boundary() {
    assert!(OciArtifact::parse("registry.example/service:latest").is_err());
}

#[test]
fn deploys_with_progress_events_in_the_same_semantic_order_and_the_same_result() {
    let database_path = temporary_database_path();
    let mut connection = database::open(&database_path).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("another"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/team/service@{digest}");
    let artifact = OciArtifact::parse(&reference).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let environment = FakePodman::new(port);
    let server = thread::spawn(move || respond_once(&listener));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let mut report = move |event: DeploymentProgress| sink.borrow_mut().push(event);

    let deployed = environment.run(|| {
        deploy_oci_with_progress(
            &mut connection,
            &application.id,
            &artifact,
            None,
            None,
            &mut report,
        )
    });
    server.join().unwrap();

    let deployed = deployed.unwrap();

    // The reported run must persist the same outcome as the silent deployment contract.
    let (status, image_reference): (String, String) = connection
        .query_row(
            "SELECT d.status, r.image_reference
             FROM deployments d
             JOIN releases r ON r.id = d.release_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "succeeded");
    assert_eq!(image_reference, reference);
    assert_eq!(deployed.artifact.reference(), reference);
    assert!(deployed.source_revision.is_none());

    let observed: Vec<EventShape> = events.borrow().iter().map(event_shape).collect();
    assert_eq!(observed, internal_deployment_progress_sequence());
}

#[test]
fn rejects_a_mismatched_repository_identically_while_reporting_nothing() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("another"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/other/service@{digest}");
    let artifact = OciArtifact::parse(&reference).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let mut report = move |event: DeploymentProgress| sink.borrow_mut().push(event);

    let error = deploy_oci_with_progress(
        &mut connection,
        &application.id,
        &artifact,
        None,
        None,
        &mut report,
    )
    .unwrap_err();

    // Delivery validation stays ahead of every external effect and of any progress event.
    assert!(matches!(
        error,
        DeployOciError::RepositoryMismatch { allowed, actual, .. }
            if allowed == "registry.example/team/service"
                && actual == "registry.example/other/service"
    ));
    assert!(events.borrow().is_empty());
}

#[test]
fn rejects_a_repository_not_allowed_by_the_delivery_spec_before_pull() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application = import_application(
        &mut connection,
        &fixture_path("another"),
        None,
        "https://example.test/app.git",
        None,
    )
    .unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/other/service@{digest}");
    let artifact = OciArtifact::parse(&reference).unwrap();

    let error = deploy_oci(&mut connection, &application.id, &artifact, None, None).unwrap_err();

    assert!(matches!(
        error,
        DeployOciError::RepositoryMismatch {
            allowed,
            actual,
            ..
        } if allowed == "registry.example/team/service" && actual == "registry.example/other/service"
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
            "pneuma-deploy-oci-{}",
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
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$PNEUMA_FAKE_PODMAN_LOG\"\ncase \"$1\" in\n  pull|start|container) ;;\n  image) printf '%s\\n' \"${PNEUMA_FAKE_DIGEST}\" ;;\n  inspect) if [ \"$2\" = \"--format\" ] && [ \"$3\" = \"{{.Id}}\" ]; then printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'; else printf 'running\\n'; fi ;;\n  port) printf '127.0.0.1:%s\\n' \"$PNEUMA_FAKE_PORT\" ;;\n  *) exit 1 ;;\nesac\n",
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
        let result = operation();
        if let Some(path) = previous_path {
            unsafe { env::set_var("PATH", path) };
        }
        result
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

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

// Semantic progress shape: step boundaries and persisted state transitions without detail text.
#[derive(Debug, PartialEq, Eq)]
enum EventShape {
    Started(DeploymentStep),
    Completed(DeploymentStep),
    StateChanged(DeploymentStatus),
    FailurePersisted,
}

fn event_shape(event: &DeploymentProgress) -> EventShape {
    match event {
        DeploymentProgress::StepStarted { step, .. } => EventShape::Started(*step),
        DeploymentProgress::StepCompleted { step, .. } => EventShape::Completed(*step),
        DeploymentProgress::StateChanged { status, .. } => EventShape::StateChanged(*status),
        DeploymentProgress::FailurePersisted { .. } => EventShape::FailurePersisted,
    }
}

// The milestone order every internal deployment reports, regardless of reporting being enabled.
fn internal_deployment_progress_sequence() -> Vec<EventShape> {
    vec![
        EventShape::Started(DeploymentStep::LoadSpecification),
        EventShape::Completed(DeploymentStep::LoadSpecification),
        EventShape::Started(DeploymentStep::CreateDeployment),
        EventShape::Completed(DeploymentStep::CreateDeployment),
        EventShape::StateChanged(DeploymentStatus::Pending),
        EventShape::StateChanged(DeploymentStatus::Starting),
        EventShape::Started(DeploymentStep::CreateContainer),
        EventShape::Completed(DeploymentStep::CreateContainer),
        EventShape::Completed(DeploymentStep::StartContainer),
        EventShape::Completed(DeploymentStep::ObserveContainer),
        EventShape::Completed(DeploymentStep::RegisterCandidate),
        EventShape::StateChanged(DeploymentStatus::Verifying),
        EventShape::Started(DeploymentStep::HealthCheckAndPromotion),
        EventShape::Completed(DeploymentStep::HealthCheckAndPromotion),
        EventShape::StateChanged(DeploymentStatus::Succeeded),
    ]
}

fn temporary_database_path() -> PathBuf {
    env::temp_dir().join(format!(
        "pneuma-deploy-oci-{}.sqlite3",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
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
