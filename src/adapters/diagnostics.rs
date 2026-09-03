use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::Connection;

use crate::adapters::database;
use crate::adapters::oci_image::pull_image;
use crate::adapters::stores::release_store;
use crate::domain::release::OciArtifact;

const QUADLET_GENERATOR_CANDIDATES: &[&str] = &[
    "/usr/lib/systemd/user-generators/podman-user-generator",
    "/lib/systemd/user-generators/podman-user-generator",
];

/// One observable doctor check and its unformatted result.
#[derive(Debug)]
pub enum DoctorCheck {
    DatabaseConnection(CheckOutcome),
    DatabaseSchema(CheckOutcome),
    WorkspaceDirectory {
        path: PathBuf,
        exists: bool,
    },
    CaddyManagedDirectory {
        path: PathBuf,
        exists: bool,
    },
    Caddyfile {
        path: PathBuf,
        exists: bool,
    },
    CaddyConfiguration(CheckOutcome),
    Git(CheckOutcome),
    Podman(CheckOutcome),
    ActiveOciImage {
        image: String,
        outcome: CheckOutcome,
    },
    ActiveOciImages(CheckOutcome),
    ActiveLocalImage,
    DiskSpace {
        path: PathBuf,
        outcome: CheckOutcome,
    },
    PodmanRootless(CheckOutcome),
    PodmanQuadletUserGenerator {
        path: Option<PathBuf>,
    },
    Caddy(CheckOutcome),
}

impl DoctorCheck {
    pub fn is_passing(&self) -> bool {
        match self {
            Self::DatabaseConnection(outcome)
            | Self::DatabaseSchema(outcome)
            | Self::CaddyConfiguration(outcome)
            | Self::Git(outcome)
            | Self::Podman(outcome)
            | Self::PodmanRootless(outcome)
            | Self::Caddy(outcome) => outcome.is_passing(),
            Self::WorkspaceDirectory { exists, .. }
            | Self::CaddyManagedDirectory { exists, .. }
            | Self::Caddyfile { exists, .. } => *exists,
            Self::ActiveOciImage { outcome, .. } => outcome.is_passing(),
            Self::ActiveOciImages(outcome) => outcome.is_passing(),
            Self::ActiveLocalImage => true,
            Self::DiskSpace { outcome, .. } => outcome.is_passing(),
            Self::PodmanQuadletUserGenerator { path } => path.is_some(),
        }
    }

    pub fn verbose_label(&self) -> Option<&'static str> {
        match self {
            Self::DatabaseConnection(_) => Some("checking database connection"),
            Self::DatabaseSchema(_) => Some("checking database schema"),
            Self::WorkspaceDirectory { .. } => Some("checking workspace directory"),
            Self::CaddyManagedDirectory { .. } => Some("checking Caddy managed directory"),
            Self::Caddyfile { .. } => Some("checking Caddyfile"),
            Self::Git(_) => Some("checking Git availability"),
            Self::Podman(_) => Some("checking Podman availability"),
            Self::PodmanQuadletUserGenerator { .. } => {
                Some("checking Podman Quadlet user generator")
            }
            Self::Caddy(_) => Some("checking Caddy availability"),
            Self::CaddyConfiguration(_)
            | Self::ActiveOciImage { .. }
            | Self::ActiveOciImages(_)
            | Self::ActiveLocalImage
            | Self::DiskSpace { .. }
            | Self::PodmanRootless(_) => None,
        }
    }
}

/// The outcome of one doctor check. Detail is operational evidence, not UI text.
#[derive(Debug)]
pub enum CheckOutcome {
    Passed { detail: String },
    Failed { detail: String },
    Unavailable { detail: String },
}

impl CheckOutcome {
    fn passed(detail: impl Into<String>) -> Self {
        Self::Passed {
            detail: detail.into(),
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self::Failed {
            detail: detail.into(),
        }
    }

    fn unavailable(detail: impl Into<String>) -> Self {
        Self::Unavailable {
            detail: detail.into(),
        }
    }

    pub fn is_passing(&self) -> bool {
        matches!(self, Self::Passed { .. })
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Passed { detail } | Self::Failed { detail } | Self::Unavailable { detail } => {
                detail
            }
        }
    }
}

/// Typed diagnostic result returned to an interface for rendering.
#[derive(Debug)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn database_connection_failure(database_path: &Path) -> Self {
        Self {
            checks: vec![DoctorCheck::DatabaseConnection(CheckOutcome::failed(
                format!("unable to open database at {}", database_path.display()),
            ))],
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.checks.iter().all(DoctorCheck::is_passing)
    }
}

// Performs direct host observations and returns their typed results in command order.
pub fn run(
    connection: &Connection,
    database_path: &Path,
    workspace_path: &Path,
    caddy_managed_path: &Path,
    caddyfile_path: &Path,
) -> DoctorReport {
    let mut checks = vec![
        DoctorCheck::DatabaseConnection(match connection.query_row("SELECT 1", [], |_| Ok(())) {
            Ok(()) => CheckOutcome::passed("OK"),
            Err(source) => CheckOutcome::failed(source.to_string()),
        }),
        DoctorCheck::DatabaseSchema(match database::migration_identity(connection) {
            Ok(identity) => CheckOutcome::passed(identity),
            Err(source) => CheckOutcome::failed(source.to_string()),
        }),
        DoctorCheck::WorkspaceDirectory {
            path: workspace_path.to_path_buf(),
            exists: workspace_path.exists(),
        },
        DoctorCheck::CaddyManagedDirectory {
            path: caddy_managed_path.to_path_buf(),
            exists: caddy_managed_path.exists(),
        },
    ];

    let caddyfile_exists = caddyfile_path.exists();
    checks.push(DoctorCheck::Caddyfile {
        path: caddyfile_path.to_path_buf(),
        exists: caddyfile_exists,
    });
    if caddyfile_exists {
        checks.push(DoctorCheck::CaddyConfiguration(
            caddy_configuration_outcome(
                Command::new("caddy")
                    .args(["validate", "--config"])
                    .arg(caddyfile_path)
                    .args(["--adapter", "caddyfile"])
                    .output(),
            ),
        ));
    }

    checks.push(DoctorCheck::Git(command_outcome(
        Command::new("git").arg("--version").output(),
        "command failed",
    )));
    checks.push(DoctorCheck::Podman(command_outcome(
        Command::new("podman").arg("--version").output(),
        "command failed",
    )));

    match release_store::active_application_image_references(connection) {
        Ok(images) => {
            for image in images {
                if let Ok(artifact) = OciArtifact::parse(&image) {
                    let outcome = match pull_image(&artifact) {
                        Ok(_) => CheckOutcome::passed("pullable"),
                        Err(source) => CheckOutcome::failed(source.to_string()),
                    };
                    checks.push(DoctorCheck::ActiveOciImage { image, outcome });
                } else {
                    checks.push(DoctorCheck::ActiveLocalImage);
                }
            }
        }
        Err(source) => checks.push(DoctorCheck::ActiveOciImages(CheckOutcome::failed(
            source.to_string(),
        ))),
    }

    for path in [database_path, workspace_path] {
        checks.push(DoctorCheck::DiskSpace {
            path: path.to_path_buf(),
            outcome: disk_space_outcome(path),
        });
    }

    checks.push(DoctorCheck::PodmanRootless(rootless_outcome(
        Command::new("podman")
            .args(["info", "--format", "{{.Host.Security.Rootless}}"])
            .output(),
    )));
    if let Some(DoctorCheck::PodmanRootless(outcome)) = checks.last_mut() {
        if outcome.is_passing() && outcome.detail().trim() != "true" {
            *outcome = CheckOutcome::failed(outcome.detail().to_owned());
        }
    }

    checks.push(DoctorCheck::PodmanQuadletUserGenerator {
        path: QUADLET_GENERATOR_CANDIDATES
            .iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .map(Path::to_path_buf),
    });
    checks.push(DoctorCheck::Caddy(command_outcome(
        Command::new("caddy").arg("version").output(),
        "command failed",
    )));

    DoctorReport { checks }
}

fn command_outcome(
    output: Result<std::process::Output, std::io::Error>,
    failed_detail: &str,
) -> CheckOutcome {
    match output {
        Ok(output) if output.status.success() => {
            CheckOutcome::passed(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            if stderr.is_empty() {
                CheckOutcome::failed(failed_detail)
            } else {
                CheckOutcome::failed(stderr)
            }
        }
        Err(source) => CheckOutcome::unavailable(source.to_string()),
    }
}

fn caddy_configuration_outcome(
    output: Result<std::process::Output, std::io::Error>,
) -> CheckOutcome {
    match output {
        Ok(output) if output.status.success() => CheckOutcome::passed("valid"),
        Ok(output) => CheckOutcome::failed(String::from_utf8_lossy(&output.stderr).into_owned()),
        Err(source) => CheckOutcome::unavailable(source.to_string()),
    }
}

fn rootless_outcome(output: Result<std::process::Output, std::io::Error>) -> CheckOutcome {
    match output {
        Ok(output) if output.status.success() => {
            CheckOutcome::passed(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => CheckOutcome::failed(String::from_utf8_lossy(&output.stdout).into_owned()),
        Err(source) => CheckOutcome::unavailable(source.to_string()),
    }
}

fn disk_space_outcome(path: &Path) -> CheckOutcome {
    let output = match Command::new("df").args(["-Pk"]).arg(path).output() {
        Ok(output) => output,
        Err(_) => return CheckOutcome::unavailable("unable to inspect"),
    };
    if !output.status.success() {
        return CheckOutcome::unavailable("unable to inspect");
    }
    let sufficient = String::from_utf8_lossy(&output.stdout)
        .lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|value| value >= 1024 * 1024);
    if sufficient {
        CheckOutcome::passed("at least 1 GiB free")
    } else {
        CheckOutcome::failed("less than 1 GiB free")
    }
}
