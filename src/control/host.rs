use std::path::PathBuf;

use crate::adapters::database;

/// Immutable host configuration captured once per executor. It grows as more
/// command families route through the boundary; adapters currently resolve
/// their own remaining paths from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfiguration {
    pub database_path: PathBuf,
}

impl HostConfiguration {
    // Builds host configuration from an explicit database path.
    pub fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }

    // Resolves the documented `PNEUMA_DATABASE_PATH` configuration.
    pub fn from_environment() -> Self {
        Self::new(database::configured_path())
    }
}
