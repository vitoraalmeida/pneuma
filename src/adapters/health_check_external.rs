use std::io;
use std::process::Command;

use thiserror::Error;

use crate::domain::exposure::DomainName;
use crate::domain::runtime::{HealthCheckPath, HealthCheckStatus};

#[derive(Debug, PartialEq, Eq)]
// Captures the confirmed public HTTP status for exposure materialization evidence.
pub(crate) struct ExternalHealthCheck {
    pub(crate) response_status: u16,
}

#[derive(Debug, Error)]
pub enum ExternalHealthCheckError {
    #[error("failed to execute external health check: {source}")]
    Execute {
        #[source]
        source: io::Error,
    },
    #[error("external HTTPS request failed: {}", stderr.trim())]
    RequestFailed { stderr: String },
    #[error("external health check returned an invalid status: {}", stdout.trim())]
    InvalidResponse { stdout: String },
    #[error("external health check expected HTTP {expected}, got {actual}")]
    UnexpectedStatus { expected: u16, actual: u16 },
}

// Checks the public HTTPS listener through local Caddy, verifying TLS and routing without external DNS.
pub(crate) fn check_external_health(
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
        .and_then(|status| HealthCheckStatus::new(status).ok());
    let Some(response_status) = response_status else {
        return Err(ExternalHealthCheckError::InvalidResponse { stdout });
    };
    if response_status.get() != expected_status.get() {
        return Err(ExternalHealthCheckError::UnexpectedStatus {
            expected: expected_status.get(),
            actual: response_status.get(),
        });
    }

    Ok(ExternalHealthCheck {
        response_status: response_status.get(),
    })
}
