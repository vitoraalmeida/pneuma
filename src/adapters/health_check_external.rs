use std::error::Error;
use std::fmt;
use std::io;
use std::process::Command;

use crate::domain::exposure::DomainName;
use crate::domain::runtime::{HealthCheckPath, HealthCheckStatus};

#[derive(Debug, PartialEq, Eq)]
// Captures the confirmed public HTTP status for exposure materialization evidence.
pub struct ExternalHealthCheck {
    pub response_status: u16,
}

#[derive(Debug)]
pub enum ExternalHealthCheckError {
    Execute { source: io::Error },
    RequestFailed { stderr: String },
    InvalidResponse { stdout: String },
    UnexpectedStatus { expected: u16, actual: u16 },
}

impl fmt::Display for ExternalHealthCheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Execute { source } => {
                write!(
                    formatter,
                    "failed to execute external health check: {source}"
                )
            }
            Self::RequestFailed { stderr } => write!(
                formatter,
                "external HTTPS request failed: {}",
                stderr.trim()
            ),
            Self::InvalidResponse { stdout } => write!(
                formatter,
                "external health check returned an invalid status: {}",
                stdout.trim()
            ),
            Self::UnexpectedStatus { expected, actual } => write!(
                formatter,
                "external health check expected HTTP {expected}, got {actual}"
            ),
        }
    }
}

impl Error for ExternalHealthCheckError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execute { source } => Some(source),
            Self::RequestFailed { .. }
            | Self::InvalidResponse { .. }
            | Self::UnexpectedStatus { .. } => None,
        }
    }
}

// Checks the public HTTPS listener through local Caddy, verifying TLS and routing without external DNS.
pub fn check_external_health(
    domain: &DomainName,
    path: &HealthCheckPath,
    expected_status: HealthCheckStatus,
) -> Result<ExternalHealthCheck, ExternalHealthCheckError> {
    let domain = domain.as_str();
    let path = path.as_str();
    let url = format!("https://{domain}{path}");
    let resolve = format!("{domain}:443:127.0.0.1");
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            "--connect-timeout",
            "2",
            "--max-time",
            "2",
            // Caddy may provision a first certificate asynchronously after the
            // route reload. Keep the candidate route in place long enough for
            // ACME validation and certificate download to finish.
            "--retry",
            "30",
            "--retry-delay",
            "1",
            "--retry-all-errors",
            "--noproxy",
            "*",
            "--resolve",
            &resolve,
            &url,
        ])
        .output()
        .map_err(|source| ExternalHealthCheckError::Execute { source })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(ExternalHealthCheckError::RequestFailed { stderr });
    }
    let response_status = stdout
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|status| (100..=599).contains(status));
    let Some(response_status) = response_status else {
        return Err(ExternalHealthCheckError::InvalidResponse { stdout });
    };
    if response_status != expected_status.get() {
        return Err(ExternalHealthCheckError::UnexpectedStatus {
            expected: expected_status.get(),
            actual: response_status,
        });
    }

    Ok(ExternalHealthCheck { response_status })
}
