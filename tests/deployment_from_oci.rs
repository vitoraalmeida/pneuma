use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::database;
use pneuma::domain::deployment::SourceRevision;
use pneuma::domain::identity::ApplicationId;
use pneuma::use_cases::application_import::import_application;
use pneuma::use_cases::deployment_from_oci::{DeployOciError, deploy_oci};

#[test]
fn deploys_a_verified_oci_image_and_persists_its_exact_reference() {
    let database_path = temporary_database_path();
    let mut connection = database::open(&database_path).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("another"), None, None, None).unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/team/service@{digest}");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let environment = FakePodman::new(port);
    let server = thread::spawn(move || respond_once(&listener));

    let deployed =
        environment.run(|| deploy_oci(&mut connection, &application.id, &reference, None, None));
    server.join().unwrap();

    let release = connection
        .query_row(
            "SELECT r.id, r.image_reference, r.image_repository, r.image_digest, d.source_revision
             FROM releases r
             JOIN deployments d ON d.release_id = r.id",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .unwrap();
    let deployed = deployed.unwrap();

    assert_eq!(release.1, reference);
    assert_eq!(release.2, "registry.example/team/service");
    assert_eq!(release.3, digest);
    assert_eq!(release.4, None);
    assert_eq!(deployed.artifact.reference(), release.1);
    assert_eq!(
        deployed
            .source_revision
            .as_ref()
            .map(SourceRevision::as_str),
        release.4.as_deref()
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
fn rejects_an_unpinned_oci_reference_before_external_work() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();

    let error = deploy_oci(
        &mut connection,
        &ApplicationId::from("application"),
        "registry.example/service:latest",
        None,
        None,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        DeployOciError::PullImage {
            source: pneuma::adapters::oci_image::PullImageError::InvalidReference { .. }
        }
    ));
}

#[test]
fn rejects_a_repository_not_allowed_by_the_delivery_spec_before_pull() {
    let mut connection = database::open(Path::new(":memory:")).unwrap();
    let application =
        import_application(&mut connection, &fixture_path("another"), None, None, None).unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = format!("registry.example/other/service@{digest}");

    let error = deploy_oci(&mut connection, &application.id, &reference, None, None).unwrap_err();

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
