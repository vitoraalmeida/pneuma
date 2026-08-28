use rusqlite::Connection;

use crate::adapters::stores::system_store;
use crate::domain::system::{System, SystemName};

// Creates a System once and returns the existing row when its name already exists.
pub fn create_system(
    connection: &mut Connection,
    name: &SystemName,
    description: Option<&str>,
) -> Result<System, rusqlite::Error> {
    let transaction = connection.transaction()?;

    let system = system_store::create_or_load(&transaction, name, description)?;

    transaction.commit()?;

    Ok(system)
}
