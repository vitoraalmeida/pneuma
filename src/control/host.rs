use std::path::PathBuf;

use crate::adapters::database;
use crate::config::{
    CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE, CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
    DEFAULT_CADDY_MANAGED_PATH, DEFAULT_CADDYFILE_PATH, DEFAULT_WORKSPACE_PATH,
    WORKSPACE_PATH_ENVIRONMENT_VARIABLE, configured_path,
};

/// Immutable host configuration captured once per executor. It grows as more
/// command families route through the boundary; adapters currently resolve
/// their own remaining paths from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfiguration {
    pub database_path: PathBuf,
    // Workspace root for isolated import checkouts.
    pub workspace_path: PathBuf,
    // Caddy route fragments managed by exposure and reconciliation commands.
    pub caddy_managed_path: PathBuf,
    // Main Caddyfile the fragments are validated and reloaded against.
    pub caddyfile_path: PathBuf,
}

impl HostConfiguration {
    // Builds host configuration from explicit paths.
    pub fn new(
        database_path: PathBuf,
        workspace_path: PathBuf,
        caddy_managed_path: PathBuf,
        caddyfile_path: PathBuf,
    ) -> Self {
        Self {
            database_path,
            workspace_path,
            caddy_managed_path,
            caddyfile_path,
        }
    }

    // Resolves the documented `PNEUMA_DATABASE_PATH`, `PNEUMA_WORKSPACE_PATH`,
    // `PNEUMA_CADDY_MANAGED_PATH`, and `PNEUMA_CADDYFILE_PATH`.
    pub fn from_environment() -> Self {
        Self::new(
            database::configured_path(),
            configured_path(WORKSPACE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_WORKSPACE_PATH),
            configured_path(
                CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
                DEFAULT_CADDY_MANAGED_PATH,
            ),
            configured_path(CADDYFILE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_CADDYFILE_PATH),
        )
    }
}
