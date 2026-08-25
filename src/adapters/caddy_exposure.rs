use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use crate::domain::exposure::DomainName;
use crate::domain::identity::ApplicationId;
use crate::domain::reconciliation::CaddyFragmentObservation;
use crate::domain::runtime::ExpectedRuntimeEndpoint;

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

#[derive(Debug, Error)]
pub enum CaddyCommandError {
    #[error("could not execute Caddy: {source}")]
    Execute {
        #[source]
        source: io::Error,
    },
    #[error("Caddy rejected the command: {}", diagnostic(stdout, stderr))]
    Rejected { stdout: String, stderr: String },
}

// `ValidateConfiguration` appends a recovery suffix only when recovery was
// attempted, so `Display` stays hand-written while the derive supplies the
// source chain.
#[derive(Debug, Error)]
pub enum CaddyRecoveryError {
    RestoreFragment {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    Reload {
        #[source]
        failure: CaddyCommandError,
    },
    ValidateConfiguration {
        #[source]
        failure: CaddyCommandError,
        recovery: Option<Box<CaddyRecoveryError>>,
    },
    ReloadRecovery {
        #[source]
        failure: CaddyCommandError,
        recovery: Box<CaddyRecoveryError>,
    },
}

#[derive(Debug, PartialEq, Eq)]
// Adapter DTO: the next filesystem/Caddy step to perform while materializing
// or removing a route; execution owns the actual external effects.
pub enum CaddyFilesystemAction {
    InspectCaddyfile,
    CreateManagedDirectory,
    ReadPreviousFragment,
    WriteTemporaryFragment,
    ActivateFragment,
}

#[derive(Debug, Error)]
pub enum ObserveCaddyFragmentError {
    #[error("application ID must be a 32-character hexadecimal value")]
    InvalidApplicationId,
    #[error("failed to read Caddy fragment at {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

// `ValidateConfiguration` and `Reload` share the recovery suffix helper in
// `Display`, so the derive only supplies the source chain.
#[derive(Debug, Error)]
pub enum MaterializeCaddyFragmentError {
    InvalidApplicationId,
    InvalidCaddyfile {
        path: PathBuf,
    },
    Filesystem {
        action: CaddyFilesystemAction,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    ValidateFragment {
        #[source]
        failure: CaddyCommandError,
    },
    ValidateConfiguration {
        #[source]
        failure: CaddyCommandError,
        recovery: Option<Box<CaddyRecoveryError>>,
    },
    Reload {
        #[source]
        failure: CaddyCommandError,
        recovery: Option<Box<CaddyRecoveryError>>,
    },
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

impl fmt::Display for MaterializeCaddyFragmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidApplicationId => {
                formatter.write_str("application ID must be a 32-character hexadecimal value")
            }
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
                write_recovery(formatter, recovery.as_deref())
            }
            Self::Reload { failure, recovery } => {
                write!(formatter, "failed to reload Caddy: {failure}")?;
                write_recovery(formatter, recovery.as_deref())
            }
        }
    }
}

impl MaterializeCaddyFragmentError {
    // Signals whether recovery failed, requiring callers to record route divergence.
    pub(crate) fn recovery_failed(&self) -> bool {
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
    application_id: &ApplicationId,
    domain: &DomainName,
    endpoint: ExpectedRuntimeEndpoint,
) -> Result<MaterializedCaddyFragment, MaterializeCaddyFragmentError> {
    if !is_safe_fragment_stem(application_id.as_str()) {
        return Err(MaterializeCaddyFragmentError::InvalidApplicationId);
    }
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
    let fragment_path = managed_directory.join(format!("{}.caddy", application_id.as_str()));
    let temporary_path = managed_directory.join(format!(".{}.caddy.tmp", application_id.as_str()));
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
            let recovery = restore_fragment(
                &fragment_path,
                &temporary_path,
                previous_fragment.as_deref(),
            )
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
                previous_fragment.as_deref(),
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
pub fn canonical_fragment_contents(
    domain: &DomainName,
    endpoint: ExpectedRuntimeEndpoint,
) -> String {
    format!(
        "{} {{\n    reverse_proxy {}\n}}\n",
        domain.as_str(),
        endpoint.socket_addr()
    )
}

// Reads one managed fragment without creating directories, validating, or reloading Caddy.
pub fn observe_caddy_fragment(
    managed_directory: &Path,
    application_id: &ApplicationId,
) -> Result<CaddyFragmentObservation, ObserveCaddyFragmentError> {
    if !is_safe_fragment_stem(application_id.as_str()) {
        return Err(ObserveCaddyFragmentError::InvalidApplicationId);
    }
    let path = managed_directory.join(format!("{}.caddy", application_id.as_str()));
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
        materialized.previous_fragment.as_deref(),
        caddyfile_path,
    )
}

// Removes an application's managed route and reloads Caddy, retaining the previous fragment for recovery.
pub fn remove_caddy_fragment(
    managed_directory: &Path,
    application_id: &ApplicationId,
    caddyfile_path: &Path,
) -> Result<RemovedCaddyFragment, CaddyRecoveryError> {
    let fragment_path = managed_directory.join(format!("{}.caddy", application_id.as_str()));
    let temporary_path = managed_directory.join(format!(".{}.caddy.tmp", application_id.as_str()));
    let previous_fragment =
        read_previous_fragment(&fragment_path).map_err(|error| match error {
            MaterializeCaddyFragmentError::Filesystem { path, source, .. } => {
                CaddyRecoveryError::RestoreFragment { path, source }
            }
            _ => unreachable!(),
        })?;
    restore_fragment(&fragment_path, &temporary_path, None)?;
    if let Err(failure) = caddy_command("validate", caddyfile_path) {
        let recovery = restore_fragment(
            &fragment_path,
            &temporary_path,
            previous_fragment.as_deref(),
        )
        .err()
        .map(Box::new);
        return Err(CaddyRecoveryError::ValidateConfiguration { failure, recovery });
    }
    if let Err(failure) = caddy_command("reload", caddyfile_path) {
        let recovery = recover_previous_configuration(
            &fragment_path,
            &temporary_path,
            previous_fragment.as_deref(),
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
pub(crate) fn restore_removed_caddy_fragment(
    removed: &RemovedCaddyFragment,
    caddyfile_path: &Path,
) -> Result<(), CaddyRecoveryError> {
    recover_previous_configuration(
        &removed.path,
        &removed.temporary_path,
        removed.previous_fragment.as_deref(),
        caddyfile_path,
    )
}

impl CaddyRecoveryError {
    // Signals whether a failed removal left the route in an unconfirmed state.
    pub(crate) fn recovery_failed(&self) -> bool {
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

// Ensures the application identity is safe to embed in managed Caddy file names.
fn is_safe_fragment_stem(application_id: &str) -> bool {
    application_id.len() == 32 && application_id.bytes().all(|byte| byte.is_ascii_hexdigit())
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
    previous_fragment: Option<&[u8]>,
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
    previous_fragment: Option<&[u8]>,
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
    recovery: Option<&CaddyRecoveryError>,
) -> fmt::Result {
    if let Some(recovery) = recovery {
        write!(formatter, "; recovery also failed: {recovery}")?;
    }
    Ok(())
}
