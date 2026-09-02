use std::io::{self, Write};

// Writes one best-effort stderr line: presentation failures never propagate.
pub(crate) fn write_stderr_line(message: impl std::fmt::Display) {
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{message}");
}

// Emits operational detail only when the global verbose flag is enabled.
pub(crate) fn log_verbose(verbose: bool, message: impl std::fmt::Display) {
    if verbose {
        write_stderr_line(format_args!("[verbose] {message}"));
    }
}
