use std::process::ExitCode;

use pneuma::host_environment::configure_startup_environment;

mod cli;

// Initializes process-wide environment before parsing and dispatching the CLI request.
fn main() -> ExitCode {
    if let Err(error) = configure_startup_environment() {
        eprintln!("error: {error}");
        return ExitCode::from(1);
    }

    let result = cli::parse_invocation().and_then(cli::run);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.class().exit_code())
        }
    }
}
