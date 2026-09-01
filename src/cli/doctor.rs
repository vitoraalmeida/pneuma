use pneuma::control::{Command, CommandResult, ControlError, ControlExecutor};

use super::error::CliError;
use super::output;
use super::shared::log_verbose;

// Renders the typed doctor report and preserves the command's diagnostic failure vocabulary.
pub(crate) fn run_doctor(executor: &ControlExecutor, verbose: bool) -> Result<(), CliError> {
    match executor.execute(Command::Doctor) {
        Ok(CommandResult::Doctor(report)) => {
            render_doctor_report(&report, verbose);
            if report.is_healthy() {
                Ok(())
            } else {
                Err(CliError::Doctor)
            }
        }
        Ok(_) => unreachable!(),
        Err(ControlError::DoctorConnection { source, report }) => {
            render_doctor_report(&report, verbose);
            Err(CliError::Database { source })
        }
        Err(source) => Err(CliError::from_control(source)),
    }
}

// Renders database commands after their effects have completed through control.
pub(crate) fn run_database_backup(
    executor: &ControlExecutor,
    path: &std::path::Path,
) -> Result<(), CliError> {
    match executor
        .execute(Command::DatabaseBackup {
            path: path.to_path_buf(),
        })
        .map_err(CliError::from_control)?
    {
        CommandResult::DatabaseBackedUp { path } => {
            println!("{}", output::database_backup(&path));
            Ok(())
        }
        _ => unreachable!(),
    }
}

// Renders restore paths after control validates and replaces the live database.
pub(crate) fn run_database_restore(
    executor: &ControlExecutor,
    path: &std::path::Path,
) -> Result<(), CliError> {
    match executor
        .execute(Command::DatabaseRestore {
            path: path.to_path_buf(),
        })
        .map_err(CliError::from_control)?
    {
        CommandResult::DatabaseRestored {
            path,
            pre_restore_path,
        } => {
            println!("{}", output::database_restore(&path, &pre_restore_path));
            Ok(())
        }
        _ => unreachable!(),
    }
}

fn render_doctor_report(report: &pneuma::adapters::diagnostics::DoctorReport, verbose: bool) {
    for check in &report.checks {
        if let Some(label) = check.verbose_label() {
            log_verbose(verbose, label);
        }
    }
    println!("{}", output::doctor_report(report));
}
