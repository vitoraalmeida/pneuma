//! Interface-neutral execution boundary.
//!
//! The [`ControlExecutor`] owns the immutable host configuration and executes
//! [`Command`]s synchronously, returning typed [`CommandResult`]s and typed
//! [`ControlError`]s. It never writes to the terminal, never detects a TTY,
//! and never selects process exit codes: presentation remains in the CLI and
//! future adapters. Every execution that needs SQLite acquires the shared
//! database-wide lock, opens one connection, performs one command, closes the
//! connection, and releases the lock.

mod command;
mod error;
mod host;
mod result;

pub use self::command::Command;
pub use self::error::ControlError;
pub use self::host::HostConfiguration;
pub use self::result::CommandResult;

use crate::adapters::database::{self, DatabaseError, DatabaseLock, LockMode};
use crate::domain::system::SystemName;
use crate::use_cases::system;

/// Executes commands against the configured host without presentation concerns.
pub struct ControlExecutor {
    host: HostConfiguration,
}

impl ControlExecutor {
    // Builds an executor over explicitly supplied host configuration.
    pub fn new(host: HostConfiguration) -> Self {
        Self { host }
    }

    // Resolves host configuration from the documented Pneuma environment variables.
    pub fn from_environment() -> Self {
        Self::new(HostConfiguration::from_environment())
    }

    pub fn host(&self) -> &HostConfiguration {
        &self.host
    }

    // Executes one command, acquiring and releasing the database-wide lock around it.
    pub fn execute(&self, command: Command) -> Result<CommandResult, ControlError> {
        let _lock = self.acquire_shared_database_lock()?;
        let mut connection = database::open(&self.host.database_path)
            .map_err(|source| ControlError::Database { source })?;

        match command {
            Command::SystemCreate { name, description } => {
                let name = SystemName::new(&name)
                    .map_err(|source| ControlError::InvalidSystemName { source })?;
                let system = system::create_system(&mut connection, &name, description.as_deref())
                    .map_err(|source| ControlError::SystemCreate { source })?;
                Ok(CommandResult::SystemCreated(system))
            }
            Command::SystemList => {
                let systems = system::list_systems(&connection)
                    .map_err(|source| ControlError::SystemList { source })?;
                Ok(CommandResult::Systems(systems))
            }
            Command::SystemShow { name } => {
                let name = SystemName::new(&name)
                    .map_err(|source| ControlError::InvalidSystemName { source })?;
                let details = system::show_system(&connection, &name)
                    .map_err(|source| ControlError::SystemShow { source })?;
                Ok(CommandResult::SystemDetails(details))
            }
        }
    }

    // Normal commands share the database-wide lock; contention is a typed busy error.
    fn acquire_shared_database_lock(&self) -> Result<DatabaseLock, ControlError> {
        match DatabaseLock::try_acquire(&self.host.database_path, LockMode::Shared) {
            Ok(Some(lock)) => Ok(lock),
            Ok(None) => Err(ControlError::Database {
                source: DatabaseError::DatabaseBusy {
                    path: self.host.database_path.clone(),
                },
            }),
            Err(source) => Err(ControlError::Database { source }),
        }
    }
}
