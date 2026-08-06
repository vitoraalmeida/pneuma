use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::manifest::is_valid_domain;

#[derive(Debug, PartialEq, Eq)]
pub struct MaterializedCaddyFragment {
    pub path: PathBuf,
    pub contents: String,
    pub validation_stdout: String,
    pub validation_stderr: String,
}

#[derive(Debug)]
pub enum MaterializeCaddyFragmentError {
    InvalidApplicationId,
    InvalidDomain,
    InvalidEndpoint { endpoint: SocketAddr },
    CreateManagedDirectory { path: PathBuf, source: io::Error },
    WriteTemporaryFragment { path: PathBuf, source: io::Error },
    ExecuteValidation { source: io::Error },
    ValidationFailed { stdout: String, stderr: String },
    ActivateFragment { path: PathBuf, source: io::Error },
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
            Self::CreateManagedDirectory { path, source } => write!(
                formatter,
                "failed to create managed Caddy directory at {}: {source}",
                path.display()
            ),
            Self::WriteTemporaryFragment { path, source } => write!(
                formatter,
                "failed to write temporary Caddy fragment at {}: {source}",
                path.display()
            ),
            Self::ExecuteValidation { source } => {
                write!(formatter, "failed to execute Caddy validation: {source}")
            }
            Self::ValidationFailed { stdout, stderr } => {
                let diagnostic = if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                };
                write!(
                    formatter,
                    "Caddy rejected the generated fragment: {diagnostic}"
                )
            }
            Self::ActivateFragment { path, source } => write!(
                formatter,
                "failed to activate Caddy fragment at {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for MaterializeCaddyFragmentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CreateManagedDirectory { source, .. }
            | Self::WriteTemporaryFragment { source, .. }
            | Self::ExecuteValidation { source }
            | Self::ActivateFragment { source, .. } => Some(source),
            Self::InvalidApplicationId
            | Self::InvalidDomain
            | Self::InvalidEndpoint { .. }
            | Self::ValidationFailed { .. } => None,
        }
    }
}

pub fn materialize_caddy_fragment(
    managed_directory: &Path,
    application_id: &str,
    domain: &str,
    endpoint: SocketAddr,
) -> Result<MaterializedCaddyFragment, MaterializeCaddyFragmentError> {
    validate_input(application_id, domain, endpoint)?;

    fs::create_dir_all(managed_directory).map_err(|source| {
        MaterializeCaddyFragmentError::CreateManagedDirectory {
            path: managed_directory.to_path_buf(),
            source,
        }
    })?;
    let fragment_path = managed_directory.join(format!("{application_id}.caddy"));
    let temporary_path = managed_directory.join(format!(".{application_id}.caddy.tmp"));
    let contents = format!("{domain} {{\n    reverse_proxy {endpoint}\n}}\n");
    fs::write(&temporary_path, &contents).map_err(|source| {
        MaterializeCaddyFragmentError::WriteTemporaryFragment {
            path: temporary_path.clone(),
            source,
        }
    })?;

    let validation = Command::new("caddy")
        .args(["validate", "--config"])
        .arg(&temporary_path)
        .args(["--adapter", "caddyfile"])
        .output();
    let validation = match validation {
        Ok(validation) => validation,
        Err(source) => {
            let _ = fs::remove_file(&temporary_path);
            return Err(MaterializeCaddyFragmentError::ExecuteValidation { source });
        }
    };
    let validation_stdout = String::from_utf8_lossy(&validation.stdout).into_owned();
    let validation_stderr = String::from_utf8_lossy(&validation.stderr).into_owned();
    if !validation.status.success() {
        let _ = fs::remove_file(&temporary_path);
        return Err(MaterializeCaddyFragmentError::ValidationFailed {
            stdout: validation_stdout,
            stderr: validation_stderr,
        });
    }

    fs::rename(&temporary_path, &fragment_path).map_err(|source| {
        let _ = fs::remove_file(&temporary_path);
        MaterializeCaddyFragmentError::ActivateFragment {
            path: fragment_path.clone(),
            source,
        }
    })?;

    Ok(MaterializedCaddyFragment {
        path: fragment_path,
        contents,
        validation_stdout,
        validation_stderr,
    })
}

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
