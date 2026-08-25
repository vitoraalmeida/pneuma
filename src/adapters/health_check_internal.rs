use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;

use crate::domain::runtime::{
    HealthCheckSpecification, HealthCheckStatus, RuntimeEndpointError, validate_loopback_endpoint,
};
use std::time::Duration;

const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_millis(500);
const MAX_ATTEMPTS: u8 = 5;
const MAX_STATUS_LINE_BYTES: u64 = 1024;

// Adapter observation result: what one internal probe saw, converted into
// typed evidence for callers; no domain decision is taken here.
#[derive(Debug, PartialEq, Eq)]
pub enum HealthCheckResult {
    Healthy {
        attempts: u8,
        response_status: u16,
    },
    Unhealthy {
        attempts: u8,
        failure: HealthCheckFailure,
    },
}

// Adapter classification of why a probe failed; timeout and unreachability are
// distinguished because retry policy treats them differently.
#[derive(Debug, PartialEq, Eq)]
pub enum HealthCheckFailure {
    TimedOut,
    Unreachable { kind: io::ErrorKind },
    InvalidResponse,
    UnexpectedStatus { expected: u16, actual: u16 },
}

impl fmt::Display for HealthCheckFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => formatter.write_str("internal health check timed out"),
            Self::Unreachable { kind } => {
                write!(
                    formatter,
                    "internal health endpoint was unreachable: {kind:?}"
                )
            }
            Self::InvalidResponse => {
                formatter.write_str("internal health endpoint returned an invalid HTTP response")
            }
            Self::UnexpectedStatus { expected, actual } => write!(
                formatter,
                "internal health endpoint returned status {actual}; expected {expected}"
            ),
        }
    }
}

// Checks a candidate's loopback endpoint with the fixed bounded retry policy used before promotion.
pub(crate) fn check_internal_health(
    endpoint: SocketAddr,
    specification: &HealthCheckSpecification,
) -> Result<HealthCheckResult, RuntimeEndpointError> {
    check_internal_health_with_policy(
        endpoint,
        specification,
        HealthCheckPolicy {
            attempt_timeout: ATTEMPT_TIMEOUT,
            retry_interval: RETRY_INTERVAL,
            max_attempts: MAX_ATTEMPTS,
        },
    )
}

#[derive(Clone, Copy)]
// Groups retry limits so production uses fixed bounds while tests can exercise timing outcomes quickly.
struct HealthCheckPolicy {
    attempt_timeout: Duration,
    retry_interval: Duration,
    max_attempts: u8,
}

// Performs retries and returns the final observed failure instead of exposing transient attempt errors.
fn check_internal_health_with_policy(
    endpoint: SocketAddr,
    specification: &HealthCheckSpecification,
    policy: HealthCheckPolicy,
) -> Result<HealthCheckResult, RuntimeEndpointError> {
    validate_loopback_endpoint(endpoint)?;

    for attempt in 1..=policy.max_attempts {
        match check_once(
            endpoint,
            specification.path().as_str(),
            specification.expected_status().get(),
            policy.attempt_timeout,
        ) {
            Ok(response_status) => {
                return Ok(HealthCheckResult::Healthy {
                    attempts: attempt,
                    response_status,
                });
            }
            Err(failure) if attempt == policy.max_attempts => {
                return Ok(HealthCheckResult::Unhealthy {
                    attempts: attempt,
                    failure,
                });
            }
            Err(_) => thread::sleep(policy.retry_interval),
        }
    }

    unreachable!("health check policy always performs at least one attempt")
}

// Sends one bounded HTTP request and parses only its status line to avoid reading an untrusted response body.
fn check_once(
    endpoint: SocketAddr,
    path: &str,
    expected_status: u16,
    timeout: Duration,
) -> Result<u16, HealthCheckFailure> {
    let mut stream = TcpStream::connect_timeout(&endpoint, timeout).map_err(classify_io_error)?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(classify_io_error)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(classify_io_error)?;

    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\n\r\n"
    )
    .map_err(classify_io_error)?;
    stream.flush().map_err(classify_io_error)?;

    let reader = BufReader::new(stream);
    let mut limited_reader = reader.take(MAX_STATUS_LINE_BYTES);
    let mut status_line = String::new();
    limited_reader
        .read_line(&mut status_line)
        .map_err(classify_io_error)?;
    if !status_line.ends_with('\n') {
        return Err(HealthCheckFailure::InvalidResponse);
    }

    let mut fields = status_line.split_whitespace();
    let protocol = fields.next();
    let response_status = fields.next().and_then(|status| status.parse::<u16>().ok());
    if !matches!(protocol, Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(HealthCheckFailure::InvalidResponse);
    }
    let response_status = response_status.and_then(|status| HealthCheckStatus::new(status).ok());
    let Some(response_status) = response_status else {
        return Err(HealthCheckFailure::InvalidResponse);
    };
    if response_status.get() != expected_status {
        return Err(HealthCheckFailure::UnexpectedStatus {
            expected: expected_status,
            actual: response_status.get(),
        });
    }

    Ok(response_status.get())
}

// Maps timeout-class I/O failures separately so retries report actionable health diagnostics.
fn classify_io_error(error: io::Error) -> HealthCheckFailure {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => HealthCheckFailure::TimedOut,
        kind => HealthCheckFailure::Unreachable { kind },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    use crate::domain::runtime::{HealthCheckPath, HealthCheckSpecification, HealthCheckStatus};

    fn health_check() -> HealthCheckSpecification {
        HealthCheckSpecification::new(
            HealthCheckPath::new("/healthz").unwrap(),
            HealthCheckStatus::new(200).unwrap(),
        )
    }

    const TEST_POLICY: HealthCheckPolicy = HealthCheckPolicy {
        attempt_timeout: Duration::from_millis(50),
        retry_interval: Duration::from_millis(1),
        max_attempts: 2,
    };

    #[test]
    fn accepts_the_expected_status() {
        let (endpoint, server) = server_with_responses(&[200]);

        let result =
            check_internal_health_with_policy(endpoint, &health_check(), TEST_POLICY).unwrap();

        server.join().unwrap();
        assert_eq!(
            result,
            HealthCheckResult::Healthy {
                attempts: 1,
                response_status: 200,
            }
        );
    }

    #[test]
    fn retries_until_the_expected_status_is_observed() {
        let (endpoint, server) = server_with_responses(&[503, 200]);

        let result =
            check_internal_health_with_policy(endpoint, &health_check(), TEST_POLICY).unwrap();

        server.join().unwrap();
        assert_eq!(
            result,
            HealthCheckResult::Healthy {
                attempts: 2,
                response_status: 200,
            }
        );
    }

    #[test]
    fn returns_the_last_unexpected_status_after_all_attempts() {
        let (endpoint, server) = server_with_responses(&[503, 502]);

        let result =
            check_internal_health_with_policy(endpoint, &health_check(), TEST_POLICY).unwrap();

        server.join().unwrap();
        assert_eq!(
            result,
            HealthCheckResult::Unhealthy {
                attempts: 2,
                failure: HealthCheckFailure::UnexpectedStatus {
                    expected: 200,
                    actual: 502,
                },
            }
        );
    }

    #[test]
    fn reports_an_unreachable_endpoint() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        drop(listener);
        let policy = HealthCheckPolicy {
            max_attempts: 1,
            ..TEST_POLICY
        };

        let result = check_internal_health_with_policy(endpoint, &health_check(), policy).unwrap();

        assert!(matches!(
            result,
            HealthCheckResult::Unhealthy {
                attempts: 1,
                failure: HealthCheckFailure::Unreachable { .. },
            }
        ));
    }

    #[test]
    fn reports_a_response_timeout() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(30));
        });
        let policy = HealthCheckPolicy {
            attempt_timeout: Duration::from_millis(5),
            max_attempts: 1,
            ..TEST_POLICY
        };

        let result = check_internal_health_with_policy(endpoint, &health_check(), policy).unwrap();

        server.join().unwrap();
        assert_eq!(
            result,
            HealthCheckResult::Unhealthy {
                attempts: 1,
                failure: HealthCheckFailure::TimedOut,
            }
        );
    }

    #[test]
    fn reports_an_invalid_http_response() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream.write_all(b"not HTTP\r\n").unwrap();
        });
        let policy = HealthCheckPolicy {
            max_attempts: 1,
            ..TEST_POLICY
        };

        let result = check_internal_health_with_policy(endpoint, &health_check(), policy).unwrap();

        server.join().unwrap();
        assert_eq!(
            result,
            HealthCheckResult::Unhealthy {
                attempts: 1,
                failure: HealthCheckFailure::InvalidResponse,
            }
        );
    }

    #[test]
    fn rejects_a_non_loopback_endpoint_before_connecting() {
        let endpoint = SocketAddr::from(([192, 0, 2, 1], 8080));

        let error = check_internal_health(endpoint, &health_check()).unwrap_err();

        assert_eq!(error, RuntimeEndpointError::NotIpv4Loopback { endpoint });
    }

    #[test]
    fn rejects_an_ipv6_loopback_endpoint_before_connecting() {
        let endpoint = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 8080));

        let error = check_internal_health(endpoint, &health_check()).unwrap_err();

        assert_eq!(error, RuntimeEndpointError::NotIpv4Loopback { endpoint });
    }

    fn server_with_responses(statuses: &[u16]) -> (SocketAddr, thread::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        let statuses = statuses.to_vec();
        let server = thread::spawn(move || {
            for status in statuses {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n");
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (endpoint, server)
    }

    fn read_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let mut buffer = [0; 1024];
            let bytes_read = stream.read(&mut buffer).unwrap();
            assert_ne!(bytes_read, 0);
            request.extend_from_slice(&buffer[..bytes_read]);
        }
    }
}
