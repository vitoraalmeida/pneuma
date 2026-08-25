use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::domain::identity::SystemId;
use crate::domain::system::System;
use crate::domain::system::SystemName;

pub(crate) fn create_or_load(
    transaction: &Transaction<'_>,
    name: &SystemName,
    description: Option<&str>,
) -> Result<System, rusqlite::Error> {
    let id: String =
        transaction.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
    transaction.execute("INSERT INTO systems (id, name, description, created_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP) ON CONFLICT(name) DO NOTHING", params![id, name.as_str(), description])?;
    transaction.query_row(
        "SELECT id, name, description FROM systems WHERE name = ?1",
        [name.as_str()],
        map_system,
    )
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<System>, rusqlite::Error> {
    let mut statement =
        connection.prepare("SELECT id, name, description FROM systems ORDER BY name")?;
    statement
        .query_map([], map_system)?
        .collect::<Result<Vec<_>, _>>()
}

pub(crate) fn load_by_name(
    connection: &Connection,
    name: &SystemName,
) -> Result<Option<System>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT id, name, description FROM systems WHERE name = ?1",
            [name.as_str()],
            map_system,
        )
        .optional()
}

fn map_system(row: &rusqlite::Row<'_>) -> rusqlite::Result<System> {
    Ok(System {
        id: SystemId::from(row.get::<_, String>(0)?),
        name: SystemName::new(&row.get::<_, String>(1)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        description: row.get(2)?,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::{TransactionBehavior, params};

    use crate::adapters::database;
    use crate::domain::system::SystemName;

    use super::{create_or_load, list, load_by_name};

    #[test]
    fn create_or_load_round_trips_a_system_and_reuses_it_by_name() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        let name = SystemName::new("team-a").unwrap();

        let first = {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            let system = create_or_load(&transaction, &name, Some("first description")).unwrap();
            transaction.commit().unwrap();
            system
        };
        let second = {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            let system = create_or_load(&transaction, &name, Some("ignored on conflict")).unwrap();
            transaction.commit().unwrap();
            system
        };

        assert_eq!(first.id, second.id);
        assert_eq!(second.name.as_str(), "team-a");

        let loaded = load_by_name(&connection, &name).unwrap().unwrap();
        assert_eq!(loaded.id, first.id);
        assert_eq!(
            loaded.description.as_deref(),
            Some("first description"),
            "the original registration must be preserved on name conflicts"
        );
        let missing =
            load_by_name(&connection, &SystemName::new("missing-system").unwrap()).unwrap();
        assert_eq!(missing, None);

        let systems = list(&connection).unwrap();
        assert_eq!(systems.len(), 1);
        assert_eq!(systems[0].id, first.id);
    }

    #[test]
    fn rejects_a_corrupt_persisted_system_name_instead_of_hydrating_it() {
        let connection = database::open(Path::new(":memory:")).unwrap();
        connection
            .execute(
                "INSERT INTO systems (id, name, description, created_at)
                 VALUES ('system-id', 'Not A Valid Name', NULL, 'now')",
                params![],
            )
            .unwrap();

        let error = list(&connection).unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::FromSqlConversionFailure(_, _, _)
        ));
    }
}
