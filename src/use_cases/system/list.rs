use rusqlite::Connection;

use crate::adapters::stores::system_store;
use crate::domain::system::System;

// Lists catalog systems in stable name order without modifying persisted state.
pub fn list_systems(connection: &Connection) -> Result<Vec<System>, rusqlite::Error> {
    system_store::list(connection)
}
