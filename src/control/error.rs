use thiserror::Error;

use crate::adapters::database::DatabaseError;
use crate::domain::system::InvalidSystemName;
use crate::use_cases::system::ShowError;

/// Typed failure of one executed command. Messages stay command-specific so
/// adapters can present them verbatim.
#[derive(Debug, Error)]
pub enum ControlError {
    #[error(transparent)]
    Database { source: DatabaseError },
    #[error(transparent)]
    InvalidSystemName { source: InvalidSystemName },
    #[error("failed to create system: {source}")]
    SystemCreate {
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to list systems: {source}")]
    SystemList {
        #[source]
        source: rusqlite::Error,
    },
    #[error(transparent)]
    SystemShow { source: ShowError },
}
