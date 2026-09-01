use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::diagnostics::DoctorCheck;
use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor, HostConfiguration};

// Executes diagnostics and database maintenance through the library boundary,
// receiving typed reports and paths without Clap or terminal output.
#[test]
fn reports_diagnostics_and_restores_a_backup_through_the_boundary() {
    let root = temporary_root("diagnostics-database");
    let database_path = root.join("pneuma.sqlite3");
    let backup_path = root.join("backup.sqlite3");
    let executor = ControlExecutor::new(HostConfiguration::new(
        database_path,
        root.join("checkouts"),
        root.join("caddy"),
        root.join("Caddyfile"),
    ));

    let report = executor.execute(Command::Doctor).unwrap();
    let CommandResult::Doctor(report) = report else {
        panic!("Doctor must yield a typed diagnostic report");
    };
    assert!(
        report
            .checks
            .iter()
            .any(|check| matches!(check, DoctorCheck::DatabaseConnection(_))),
        "the report must include database connectivity"
    );
    assert!(
        !report.is_healthy(),
        "the deliberately absent host paths must be reported as failures"
    );

    executor
        .execute(Command::SystemCreate {
            name: "before-backup".to_owned(),
            description: None,
        })
        .unwrap();
    let backed_up = executor
        .execute(Command::DatabaseBackup {
            path: backup_path.clone(),
        })
        .unwrap();
    assert!(matches!(backed_up, CommandResult::DatabaseBackedUp { .. }));
    assert!(backup_path.exists());

    executor
        .execute(Command::SystemCreate {
            name: "after-backup".to_owned(),
            description: None,
        })
        .unwrap();
    let restored = executor
        .execute(Command::DatabaseRestore {
            path: backup_path.clone(),
        })
        .unwrap();
    let CommandResult::DatabaseRestored {
        path,
        pre_restore_path,
    } = restored
    else {
        panic!("DatabaseRestore must yield the source and pre-restore paths");
    };
    assert_eq!(path, backup_path);
    assert!(pre_restore_path.exists());

    let systems = executor.execute(Command::SystemList).unwrap();
    let CommandResult::Systems(systems) = systems else {
        panic!("SystemList must yield Systems");
    };
    assert_eq!(systems.len(), 1);
    assert_eq!(systems[0].name.to_string(), "before-backup");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_a_database_open_failure_as_a_typed_doctor_error() {
    let root = temporary_root("doctor-open-failure");
    let database_path = root.join("database-directory");
    fs::create_dir(&database_path).unwrap();
    let executor = ControlExecutor::new(HostConfiguration::new(
        database_path,
        root.join("checkouts"),
        root.join("caddy"),
        root.join("Caddyfile"),
    ));

    let error = executor.execute(Command::Doctor).unwrap_err();
    let ControlError::DoctorConnection { report, .. } = error else {
        panic!("a doctor database-open failure must retain its typed report");
    };
    assert!(matches!(
        report.checks.as_slice(),
        [DoctorCheck::DatabaseConnection(_)]
    ));

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
