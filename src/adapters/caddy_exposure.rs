use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::domain::exposure::is_valid_domain;
use crate::domain::reconciliation::CaddyFragmentObservation;

#[derive(Debug, PartialEq, Eq)]
// Records a newly active fragment and the prior bytes needed to compensate a later route failure.
pub struct MaterializedCaddyFragment {
    pub path: PathBuf,
    pub contents: String,
    pub fragment_validation_stdout: String,
    pub fragment_validation_stderr: String,
    pub configuration_validation_stdout: String,
    pub configuration_validation_stderr: String,
    pub reload_stdout: String,
    pub reload_stderr: String,
    previous_fragment: Option<Vec<u8>>,
    temporary_path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
// Retains enough state to restore a managed fragment after a caller rejects its removal.
pub struct RemovedCaddyFragment {
    path: PathBuf,
    previous_fragment: Option<Vec<u8>>,
    temporary_path: PathBuf,
}

#[derive(Debug)]
pub enum CaddyCommandError {
    Execute { source: io::Error },
    Rejected { stdout: String, stderr: String },
}

#[derive(Debug)]
pub enum CaddyRecoveryError {
    RestoreFragment {
        path: PathBuf,
        source: io::Error,
    },
    Reload {
        failure: CaddyCommandError,
    },
    ValidateConfiguration {
        failure: CaddyCommandError,
        recovery: Option<Box<CaddyRecoveryError>>,
    },
    ReloadRecovery {
        failure: CaddyCommandError,
        recovery: Box<CaddyRecoveryError>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaddyFilesystemAction {
    InspectCaddyfile,
    CreateManagedDirectory,
    ReadPreviousFragment,
    WriteTemporaryFragment,
    ActivateFragment,
}

#[derive(Debug)]
pub enum ObserveCaddyFragmentError {
    InvalidApplicationId,
    Read { path: PathBuf, source: io::Error },
}

impl fmt::Display for ObserveCaddyFragmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidApplicationId => {
                formatter.write_str("application ID must be a 32-character hexadecimal value")
            }
            Self::Read { path, source } => write!(
                formatter,
                "failed to read Caddy fragment at {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ObserveCaddyFragmentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::InvalidApplicationId => None,
        }
    }
}

#[derive(Debug)]
pub enum MaterializeCaddyFragmentError {
    InvalidApplicationId,
    InvalidDomain,
    InvalidEndpoint {
        endpoint: SocketAddr,
    },
    InvalidCaddyfile {
        path: PathBuf,
    },
    Filesystem {
        action: CaddyFilesystemAction,
        path: PathBuf,
        source: io::Error,
    },
    ValidateFragment {
        failure: CaddyCommandError,
    },
    ValidateConfiguration {
        failure: CaddyCommandError,
        recovery: Option<Box<CaddyRecoveryError>>,
    },
    Reload {
        failure: CaddyCommandError,
        recovery: Option<Box<CaddyRecoveryError>>,
    },
}

impl fmt::Display for CaddyCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Execute { source } => write!(formatter, "could not execute Caddy: {source}"),
            Self::Rejected { stdout, stderr } => {
                write!(
                    formatter,
                    "Caddy rejected the command: {}",
                    diagnostic(stdout, stderr)
                )
            }
        }
    }
}

impl Error for CaddyCommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source } => Some(source),
            Self::Rejected { .. } => None,
        }
    }
}

impl fmt::Display for CaddyFilesystemAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InspectCaddyfile => "inspect the main Caddyfile at",
            Self::CreateManagedDirectory => "create the managed Caddy directory at",
            Self::ReadPreviousFragment => "read the previous Caddy fragment at",
            Self::WriteTemporaryFragment => "write the temporary Caddy fragment at",
            Self::ActivateFragment => "activate the Caddy fragment at",
        })
    }
}

impl fmt::Display for CaddyRecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RestoreFragment { path, source } => write!(
                formatter,
                "failed to restore Caddy fragment at {}: {source}",
                path.display()
            ),
            Self::Reload { failure } => {
                write!(
                    formatter,
                    "failed to reload the restored configuration: {failure}"
                )
            }
            Self::ValidateConfiguration { failure, recovery } => {
                write!(
                    formatter,
                    "failed to validate Caddy after removal: {failure}"
                )?;
                if let Some(recovery) = recovery {
                    write!(formatter, "; recovery also failed: {recovery}")?;
                }
                Ok(())
            }
            Self::ReloadRecovery { failure, recovery } => write!(
                formatter,
                "failed to reload Caddy after removal: {failure}; recovery also failed: {recovery}"
            ),
        }
    }
}

impl Error for CaddyRecoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RestoreFragment { source, .. } => Some(source),
            Self::Reload { failure } => Some(failure),
            Self::ValidateConfiguration { failure, .. } => Some(failure),
            Self::ReloadRecovery { failure, .. } => Some(failure),
        }
    }
}

impl fmt::Display for MaterializeCaddyFragmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidApplicationId => {
                formatter.write_str("application ID must be a 32-character hexadecimal value")
            }
            Self::InvalidDomain => formatter.write_str("domain must be a valid domain name"),
            Self::InvalidEndpoint { endpoint } => write!(
                formatter,
                "Caddy upstream must be a nonzero IPv4 loopback endpoint: {endpoint}"
            ),
            Self::InvalidCaddyfile { path } => write!(
                formatter,
                "main Caddyfile at {} must be a file",
                path.display()
            ),
            Self::Filesystem {
                action,
                path,
                source,
            } => write!(formatter, "failed to {action} {}: {source}", path.display()),
            Self::ValidateFragment { failure } => {
                write!(
                    formatter,
                    "failed to validate the generated fragment: {failure}"
                )
            }
            Self::ValidateConfiguration { failure, recovery } => {
                write!(
                    formatter,
                    "failed to validate the complete configuration: {failure}"
                )?;
                write_recovery(formatter, recovery)
            }
            Self::Reload { failure, recovery } => {
                write!(formatter, "failed to reload Caddy: {failure}")?;
                write_recovery(formatter, recovery)
            }
        }
    }
}

impl Error for MaterializeCaddyFragmentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } => Some(source),
            Self::ValidateFragment { failure }
            | Self::ValidateConfiguration { failure, .. }
            | Self::Reload { failure, .. } => Some(failure),
            Self::InvalidApplicationId
            | Self::InvalidDomain
            | Self::InvalidEndpoint { .. }
            | Self::InvalidCaddyfile { .. } => None,
        }
    }
}

impl MaterializeCaddyFragmentError {
    // Signals whether recovery failed, requiring callers to record route divergence.
    pub fn recovery_failed(&self) -> bool {
        matches!(
            self,
            Self::ValidateConfiguration {
                recovery: Some(_),
                ..
            } | Self::Reload {
                recovery: Some(_),
                ..
            }
        )
    }
}

// Atomically replaces an application's managed fragment, validates the full configuration, and reloads Caddy.
pub fn materialize_caddy_fragment(
    managed_directory: &Path,
    caddyfile_path: &Path,
    application_id: &str,
    domain: &str,
    endpoint: SocketAddr,
) -> Result<MaterializedCaddyFragment, MaterializeCaddyFragmentError> {
    validate_input(application_id, domain, endpoint)?;
    let caddyfile_metadata = fs::metadata(caddyfile_path).map_err(|source| {
        MaterializeCaddyFragmentError::Filesystem {
            action: CaddyFilesystemAction::InspectCaddyfile,
            path: caddyfile_path.to_path_buf(),
            source,
        }
    })?;
    if !caddyfile_metadata.is_file() {
        return Err(MaterializeCaddyFragmentError::InvalidCaddyfile {
            path: caddyfile_path.to_path_buf(),
        });
    }

    fs::create_dir_all(managed_directory).map_err(|source| {
        MaterializeCaddyFragmentError::Filesystem {
            action: CaddyFilesystemAction::CreateManagedDirectory,
            path: managed_directory.to_path_buf(),
            source,
        }
    })?;
    let fragment_path = managed_directory.join(format!("{application_id}.caddy"));
    let temporary_path = managed_directory.join(format!(".{application_id}.caddy.tmp"));
    let previous_fragment = read_previous_fragment(&fragment_path)?;
    let contents = canonical_fragment_contents(domain, endpoint);
    fs::write(&temporary_path, &contents).map_err(|source| {
        MaterializeCaddyFragmentError::Filesystem {
            action: CaddyFilesystemAction::WriteTemporaryFragment,
            path: temporary_path.clone(),
            source,
        }
    })?;

    let fragment_validation = match caddy_command("validate", &temporary_path) {
        Ok(validation) => validation,
        Err(failure) => {
            let _ = fs::remove_file(&temporary_path);
            return Err(MaterializeCaddyFragmentError::ValidateFragment { failure });
        }
    };
    let fragment_validation_stdout = fragment_validation.stdout;
    let fragment_validation_stderr = fragment_validation.stderr;

    fs::rename(&temporary_path, &fragment_path).map_err(|source| {
        let _ = fs::remove_file(&temporary_path);
        MaterializeCaddyFragmentError::Filesystem {
            action: CaddyFilesystemAction::ActivateFragment,
            path: fragment_path.clone(),
            source,
        }
    })?;

    let configuration_validation = match caddy_command("validate", caddyfile_path) {
        Ok(validation) => validation,
        Err(failure) => {
            let recovery = restore_fragment(&fragment_path, &temporary_path, &previous_fragment)
                .err()
                .map(Box::new);
            return Err(MaterializeCaddyFragmentError::ValidateConfiguration { failure, recovery });
        }
    };
    let configuration_validation_stdout = configuration_validation.stdout;
    let configuration_validation_stderr = configuration_validation.stderr;

    let reload = match caddy_command("reload", caddyfile_path) {
        Ok(reload) => reload,
        Err(failure) => {
            let recovery = recover_previous_configuration(
                &fragment_path,
                &temporary_path,
                &previous_fragment,
                caddyfile_path,
            )
            .err()
            .map(Box::new);
            return Err(MaterializeCaddyFragmentError::Reload { failure, recovery });
        }
    };
    let reload_stdout = reload.stdout;
    let reload_stderr = reload.stderr;

    Ok(MaterializedCaddyFragment {
        path: fragment_path,
        contents,
        fragment_validation_stdout,
        fragment_validation_stderr,
        configuration_validation_stdout,
        configuration_validation_stderr,
        reload_stdout,
        reload_stderr,
        previous_fragment,
        temporary_path,
    })
}

// Produces the canonical route representation used to detect and repair fragment drift.
pub fn canonical_fragment_contents(domain: &str, endpoint: SocketAddr) -> String {
    format!("{domain} {{\n    reverse_proxy {endpoint}\n}}\n")
}

// Reads one managed fragment without creating directories, validating, or reloading Caddy.
pub fn observe_caddy_fragment(
    managed_directory: &Path,
    application_id: &str,
) -> Result<CaddyFragmentObservation, ObserveCaddyFragmentError> {
    if application_id.len() != 32 || !application_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ObserveCaddyFragmentError::InvalidApplicationId);
    }
    let path = managed_directory.join(format!("{application_id}.caddy"));
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(CaddyFragmentObservation::Present { contents }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            Ok(CaddyFragmentObservation::Missing)
        }
        Err(source) => Err(ObserveCaddyFragmentError::Read { path, source }),
    }
}

// Restores the fragment active before materialization when later external health rejects the route.
pub fn restore_materialized_caddy_fragment(
    materialized: &MaterializedCaddyFragment,
    caddyfile_path: &Path,
) -> Result<(), CaddyRecoveryError> {
    recover_previous_configuration(
        &materialized.path,
        &materialized.temporary_path,
        &materialized.previous_fragment,
        caddyfile_path,
    )
}

// Removes an application's managed route and reloads Caddy, retaining the previous fragment for recovery.
pub fn remove_caddy_fragment(
    managed_directory: &Path,
    application_id: &str,
    caddyfile_path: &Path,
) -> Result<RemovedCaddyFragment, CaddyRecoveryError> {
    let fragment_path = managed_directory.join(format!("{application_id}.caddy"));
    let temporary_path = managed_directory.join(format!(".{application_id}.caddy.tmp"));
    let previous_fragment =
        read_previous_fragment(&fragment_path).map_err(|error| match error {
            MaterializeCaddyFragmentError::Filesystem { path, source, .. } => {
                CaddyRecoveryError::RestoreFragment { path, source }
            }
            _ => unreachable!(),
        })?;
    restore_fragment(&fragment_path, &temporary_path, &None)?;
    if let Err(failure) = caddy_command("validate", caddyfile_path) {
        let recovery = restore_fragment(&fragment_path, &temporary_path, &previous_fragment)
            .err()
            .map(Box::new);
        return Err(CaddyRecoveryError::ValidateConfiguration { failure, recovery });
    }
    if let Err(failure) = caddy_command("reload", caddyfile_path) {
        let recovery = recover_previous_configuration(
            &fragment_path,
            &temporary_path,
            &previous_fragment,
            caddyfile_path,
        );
        return match recovery {
            Ok(()) => Err(CaddyRecoveryError::Reload { failure }),
            Err(recovery) => Err(CaddyRecoveryError::ReloadRecovery {
                failure,
                recovery: Box::new(recovery),
            }),
        };
    }
    Ok(RemovedCaddyFragment {
        path: fragment_path,
        previous_fragment,
        temporary_path,
    })
}

// Reinstates a route removed by a caller whose subsequent operation did not complete.
pub fn restore_removed_caddy_fragment(
    removed: &RemovedCaddyFragment,
    caddyfile_path: &Path,
) -> Result<(), CaddyRecoveryError> {
    recover_previous_configuration(
        &removed.path,
        &removed.temporary_path,
        &removed.previous_fragment,
        caddyfile_path,
    )
}

impl CaddyRecoveryError {
    // Signals whether a failed removal left the route in an unconfirmed state.
    pub fn recovery_failed(&self) -> bool {
        matches!(
            self,
            Self::ReloadRecovery { .. }
                | Self::ValidateConfiguration {
                    recovery: Some(_),
                    ..
                }
        )
    }
}

// Rejects unsafe route identities and upstreams before any managed Caddy file is changed.
fn validate_input(
    application_id: &str,
    domain: &str,
    endpoint: SocketAddr,
) -> Result<(), MaterializeCaddyFragmentError> {
    if application_id.len() != 32 || !application_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(MaterializeCaddyFragmentError::InvalidApplicationId);
    }
    if !is_valid_domain(domain) {
        return Err(MaterializeCaddyFragmentError::InvalidDomain);
    }
    if endpoint.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || endpoint.port() == 0 {
        return Err(MaterializeCaddyFragmentError::InvalidEndpoint { endpoint });
    }

    Ok(())
}

// Reads the prior fragment while treating its absence as the expected first-materialization state.
fn read_previous_fragment(
    fragment_path: &Path,
) -> Result<Option<Vec<u8>>, MaterializeCaddyFragmentError> {
    match fs::read(fragment_path) {
        Ok(contents) => Ok(Some(contents)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(MaterializeCaddyFragmentError::Filesystem {
            action: CaddyFilesystemAction::ReadPreviousFragment,
            path: fragment_path.to_path_buf(),
            source,
        }),
    }
}

// Restores prior bytes atomically or removes a fragment that did not previously exist.
fn restore_fragment(
    fragment_path: &Path,
    temporary_path: &Path,
    previous_fragment: &Option<Vec<u8>>,
) -> Result<(), CaddyRecoveryError> {
    if let Some(previous_fragment) = previous_fragment {
        fs::write(temporary_path, previous_fragment).map_err(|source| {
            CaddyRecoveryError::RestoreFragment {
                path: fragment_path.to_path_buf(),
                source,
            }
        })?;
        fs::rename(temporary_path, fragment_path).map_err(|source| {
            let _ = fs::remove_file(temporary_path);
            CaddyRecoveryError::RestoreFragment {
                path: fragment_path.to_path_buf(),
                source,
            }
        })?;
    } else {
        match fs::remove_file(fragment_path) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(CaddyRecoveryError::RestoreFragment {
                    path: fragment_path.to_path_buf(),
                    source,
                });
            }
        }
        let _ = fs::remove_file(temporary_path);
    }
    Ok(())
}

// Restores the prior fragment and reloads it so filesystem and running Caddy state converge together.
fn recover_previous_configuration(
    fragment_path: &Path,
    temporary_path: &Path,
    previous_fragment: &Option<Vec<u8>>,
    caddyfile_path: &Path,
) -> Result<(), CaddyRecoveryError> {
    restore_fragment(fragment_path, temporary_path, previous_fragment)?;
    caddy_command("reload", caddyfile_path)
        .map_err(|failure| CaddyRecoveryError::Reload { failure })?;
    Ok(())
}

// Preserves successful Caddy diagnostics for callers that need materialization evidence.
struct CaddyCommandOutput {
    stdout: String,
    stderr: String,
}

// Runs Caddy through its Caddyfile adapter and retains both output streams on every outcome.
fn caddy_command(
    operation: &str,
    configuration_path: &Path,
) -> Result<CaddyCommandOutput, CaddyCommandError> {
    let output = Command::new("caddy")
        .arg(operation)
        .arg("--config")
        .arg(configuration_path)
        .args(["--adapter", "caddyfile"])
        .output()
        .map_err(|source| CaddyCommandError::Execute { source })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(CaddyCommandError::Rejected { stdout, stderr });
    }

    Ok(CaddyCommandOutput { stdout, stderr })
}

// Prefers stderr because Caddy reports failures there, falling back to stdout when necessary.
fn diagnostic<'a>(stdout: &'a str, stderr: &'a str) -> &'a str {
    if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    }
}

// Appends recovery details without obscuring the primary materialization failure.
fn write_recovery(
    formatter: &mut fmt::Formatter<'_>,
    recovery: &Option<Box<CaddyRecoveryError>>,
) -> fmt::Result {
    if let Some(recovery) = recovery {
        write!(formatter, "; recovery also failed: {recovery}")?;
    }
    Ok(())
}
