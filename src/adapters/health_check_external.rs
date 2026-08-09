use std::error::Error;
use std::fmt;
use std::io;
use std::process::Command;

use crate::domain::manifest::is_valid_domain;

#[derive(Debug, PartialEq, Eq)]
pub struct ExternalHealthCheck {
    pub response_status: u16,
}

#[derive(Debug)]
pub enum ExternalHealthCheckError {
    InvalidDomain,
    InvalidPath,
    InvalidExpectedStatus,
    Execute { source: io::Error },
    RequestFailed { stderr: String },
    InvalidResponse { stdout: String },
    UnexpectedStatus { expected: u16, actual: u16 },
}

impl fmt::Display for ExternalHealthCheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDomain => formatter.write_str("external health domain is invalid"),
            Self::InvalidPath => formatter
                .write_str("external health path must start with `/` and contain no whitespace"),
            Self::InvalidExpectedStatus => {
                formatter.write_str("expected HTTP status must be between 100 and 599")
            }
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
            Self::InvalidDomain
            | Self::InvalidPath
            | Self::InvalidExpectedStatus
            | Self::RequestFailed { .. }
            | Self::InvalidResponse { .. }
            | Self::UnexpectedStatus { .. } => None,
        }
    }
}

/// Checks the public HTTPS listener while forcing the domain to the local Caddy
/// instance. This verifies TLS, host routing, and the public path without depending
/// on external DNS propagation or sending health traffic away from the host.
pub fn check_external_health(
    domain: &str,
    path: &str,
    expected_status: u16,
) -> Result<ExternalHealthCheck, ExternalHealthCheckError> {
    if !is_valid_domain(domain) {
        return Err(ExternalHealthCheckError::InvalidDomain);
    }
    if !path.starts_with('/') || path.chars().any(char::is_whitespace) {
        return Err(ExternalHealthCheckError::InvalidPath);
    }
    if !(100..=599).contains(&expected_status) {
        return Err(ExternalHealthCheckError::InvalidExpectedStatus);
    }

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
    if response_status != expected_status {
        return Err(ExternalHealthCheckError::UnexpectedStatus {
            expected: expected_status,
            actual: response_status,
        });
    }

    Ok(ExternalHealthCheck { response_status })
}
