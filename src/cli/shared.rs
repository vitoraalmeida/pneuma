// Emits operational detail only when the global verbose flag is enabled.
pub(crate) fn log_verbose(verbose: bool, message: impl std::fmt::Display) {
    if verbose {
        eprintln!("[verbose] {message}");
    }
}
