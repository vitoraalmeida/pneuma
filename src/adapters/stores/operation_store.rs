use rusqlite::{Connection, Transaction, params};

use crate::domain::identity::ApplicationId;

#[derive(Debug, PartialEq, Eq)]
// Persistence row: the ownership token and fencing generation for one
// application's current operation (INV-DB-005); store-private coordination fact.
pub(crate) struct OperationOwnership {
    pub(crate) token: String,
    pub(crate) generation: i64,
}

// Generates an opaque owner token without inventing identity from process metadata.
pub(crate) fn generate_token(connection: &Connection) -> Result<String, rusqlite::Error> {
    connection.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
}

// Replaces the persisted owner and advances its fencing generation in the caller's short transaction.
pub(crate) fn take_ownership(
    transaction: &Transaction<'_>,
    application_id: &ApplicationId,
    token: &str,
) -> Result<OperationOwnership, rusqlite::Error> {
    transaction.query_row(
        "INSERT INTO application_operations (application_id, owner_token, generation)
             VALUES (?1, ?2, 1)
             ON CONFLICT(application_id) DO UPDATE SET
                 owner_token = excluded.owner_token,
                 generation = application_operations.generation + 1,
                 updated_at = CURRENT_TIMESTAMP
             RETURNING generation",
        params![application_id.as_str(), token],
        |row| {
            Ok(OperationOwnership {
                token: token.to_owned(),
                generation: row.get(0)?,
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::adapters::database;
    use crate::domain::identity::ApplicationId;

    use super::{generate_token, take_ownership};

    #[test]
    fn ownership_replaces_the_token_and_advances_the_generation() {
        let mut connection = database::open(Path::new(":memory:")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO applications (id, name, desired_runtime_state, spec_version, created_at, updated_at)
                 VALUES ('application', 'application', 'stopped', 1, 'now', 'now');",
            )
            .unwrap();
        let first_token = generate_token(&connection).unwrap();
        let first = {
            let transaction = connection.transaction().unwrap();
            let ownership = take_ownership(
                &transaction,
                &ApplicationId::from("application"),
                &first_token,
            )
            .unwrap();
            transaction.commit().unwrap();
            ownership
        };
        let second_token = generate_token(&connection).unwrap();
        let second = {
            let transaction = connection.transaction().unwrap();
            let ownership = take_ownership(
                &transaction,
                &ApplicationId::from("application"),
                &second_token,
            )
            .unwrap();
            transaction.commit().unwrap();
            ownership
        };

        assert_eq!(first.generation, 1);
        assert_eq!(second.generation, 2);
        assert_eq!(second.token, second_token);
    }
}
