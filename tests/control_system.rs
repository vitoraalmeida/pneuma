use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::database::{DatabaseError, DatabaseLock, LockMode};
use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor, HostConfiguration};
use pneuma::use_cases::system::ShowError;

// Executes the first command family through the library boundary without
// Clap, terminal output, or exit codes, receiving only typed results.
#[test]
fn creates_lists_and_shows_systems_through_the_boundary() {
    let root = temporary_root("system-round-trip");
    let executor = ControlExecutor::new(HostConfiguration::new(
        root.join("pneuma.sqlite3"),
        root.join("checkouts"),
    ));

    let created = executor
        .execute(Command::SystemCreate {
            name: "platform".to_owned(),
            description: Some("Team platform".to_owned()),
        })
        .unwrap();
    let CommandResult::SystemCreated(system) = created else {
        panic!("SystemCreate must yield SystemCreated");
    };

    let recreated = executor
        .execute(Command::SystemCreate {
            name: "platform".to_owned(),
            description: None,
        })
        .unwrap();
    let CommandResult::SystemCreated(recreated) = recreated else {
        panic!("SystemCreate must yield SystemCreated");
    };
    assert_eq!(recreated.id, system.id, "creation is idempotent by name");

    let listed = executor.execute(Command::SystemList).unwrap();
    let CommandResult::Systems(systems) = listed else {
        panic!("SystemList must yield Systems");
    };
    assert_eq!(systems, vec![system.clone()]);

    let shown = executor
        .execute(Command::SystemShow {
            name: "platform".to_owned(),
        })
        .unwrap();
    let CommandResult::SystemDetails(details) = shown else {
        panic!("SystemShow must yield SystemDetails");
    };
    assert_eq!(details.system, system);
    assert!(details.applications.is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn showing_a_missing_system_is_a_typed_not_found_error() {
    let root = temporary_root("system-show-missing");
    let executor = ControlExecutor::new(HostConfiguration::new(
        root.join("pneuma.sqlite3"),
        root.join("checkouts"),
    ));

    let error = executor
        .execute(Command::SystemShow {
            name: "missing".to_owned(),
        })
        .unwrap_err();

    assert!(matches!(
        error,
        ControlError::SystemShow {
            source: ShowError::NotFound { .. }
        }
    ));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_system_names_are_rejected_as_typed_input_errors() {
    let root = temporary_root("system-invalid-name");
    let executor = ControlExecutor::new(HostConfiguration::new(
        root.join("pneuma.sqlite3"),
        root.join("checkouts"),
    ));

    let error = executor
        .execute(Command::SystemCreate {
            name: "Not Valid".to_owned(),
            description: None,
        })
        .unwrap_err();

    assert!(matches!(error, ControlError::InvalidSystemName { .. }));

    let listed = executor.execute(Command::SystemList).unwrap();
    let CommandResult::Systems(systems) = listed else {
        panic!("SystemList must yield Systems");
    };
    assert!(systems.is_empty(), "a rejected name must persist no system");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_conflicting_database_holder_is_a_typed_busy_error() {
    let root = temporary_root("system-busy");
    let database_path = root.join("pneuma.sqlite3");
    let executor = ControlExecutor::new(HostConfiguration::new(
        database_path.clone(),
        root.join("checkouts"),
    ));

    let _exclusive = DatabaseLock::try_acquire(&database_path, LockMode::Exclusive)
        .unwrap()
        .unwrap();

    let error = executor.execute(Command::SystemList).unwrap_err();
    assert!(matches!(
        error,
        ControlError::Database {
            source: DatabaseError::DatabaseBusy { .. }
        }
    ));

    drop(_exclusive);

    let listed = executor.execute(Command::SystemList).unwrap();
    let CommandResult::Systems(systems) = listed else {
        panic!("SystemList must yield Systems");
    };
    assert!(systems.is_empty(), "the lock must be released after busy");

    fs::remove_dir_all(root).unwrap();
}

fn temporary_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pneuma-control-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
