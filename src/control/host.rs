use std::path::PathBuf;

use crate::adapters::database;
use crate::config::{DEFAULT_WORKSPACE_PATH, WORKSPACE_PATH_ENVIRONMENT_VARIABLE, configured_path};

/// Immutable host configuration captured once per executor. It grows as more
/// command families route through the boundary; adapters currently resolve
/// their own remaining paths from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfiguration {
    pub database_path: PathBuf,
    // Workspace root for isolated import checkouts.
    pub workspace_path: PathBuf,
}

impl HostConfiguration {
    // Builds host configuration from explicit paths.
    pub fn new(database_path: PathBuf, workspace_path: PathBuf) -> Self {
        Self {
            database_path,
            workspace_path,
        }
    }

    // Resolves the documented `PNEUMA_DATABASE_PATH` and `PNEUMA_WORKSPACE_PATH`.
    pub fn from_environment() -> Self {
        Self::new(
            database::configured_path(),
            configured_path(WORKSPACE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_WORKSPACE_PATH),
        )
    }
}
