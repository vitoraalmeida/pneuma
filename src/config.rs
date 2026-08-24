//! Single owner of the documented Pneuma path configuration and the
//! process-wide verbose logging switch shared by the CLI and host adapters.

use std::env;
use std::path::PathBuf;

pub const WORKSPACE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_WORKSPACE_PATH";
pub const DEFAULT_WORKSPACE_PATH: &str = "/var/lib/pneuma/checkouts";
pub const CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_CADDY_MANAGED_PATH";
pub const DEFAULT_CADDY_MANAGED_PATH: &str = "/etc/caddy/applications";
pub const CADDYFILE_PATH_ENVIRONMENT_VARIABLE: &str = "PNEUMA_CADDYFILE_PATH";
pub const DEFAULT_CADDYFILE_PATH: &str = "/etc/caddy/Caddyfile";

// Emits operational detail only when the global verbose flag is enabled.
pub fn log_verbose(verbose: bool, message: impl std::fmt::Display) {
    if verbose {
        eprintln!("[verbose] {message}");
    }
}

// Resolves optional path overrides consistently, treating an empty value as unset.
pub fn configured_path(variable: &str, default: &str) -> PathBuf {
    env::var_os(variable)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}
