//! Host environment bootstrap.
//!
//! Reads and validates the host environment file before any command runs,
//! applies its entries without overriding caller-supplied variables, and
//! derives the uid-scoped runtime environment. All environment mutation
//! happens here, in initial single-threaded startup.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

/// Default location of the host environment file.
pub const DEFAULT_HOST_ENVIRONMENT_FILE: &str = "/etc/pneuma/environment";

/// Environment variable overriding the host environment file location.
pub const HOST_ENVIRONMENT_FILE_VARIABLE: &str = "PNEUMA_HOST_ENVIRONMENT_FILE";

// Validates and applies the host environment file, derives the uid-scoped
// runtime environment, and enforces the Quadlet configuration requirement.
pub fn configure_startup_environment() -> Result<(), String> {
    if let Some(content) = read_host_environment_file()? {
        let entries = parse_host_environment(&content)?;
        apply_host_environment(entries);
    }

    derive_runtime_environment();
    require_quadlet_configuration()
}

// Parses `NAME=VALUE` lines, ignoring blank lines and full-line comments.
// The first `=` separates name and value, so values may be empty, contain
// additional `=` characters, and treat inline `#` as value data. The whole
// file is validated before any entry is returned; duplicates are rejected
// with both line numbers.
pub fn parse_host_environment(content: &str) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    let mut first_line_by_name: HashMap<String, usize> = HashMap::new();

    for (index, line) in content.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((name, value)) = line.split_once('=') else {
            return Err(format!(
                "invalid host environment entry on line {number}: expected NAME=VALUE"
            ));
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            return Err(format!(
                "invalid host environment entry on line {number}: empty variable name"
            ));
        }
        if !is_valid_variable_name(name) {
            return Err(format!(
                "invalid host environment entry on line {number}: invalid variable name `{name}`"
            ));
        }
        if value.contains('\0') {
            return Err(format!(
                "invalid host environment entry on line {number}: NUL byte in value"
            ));
        }
        if let Some(first) = first_line_by_name.insert(name.to_owned(), number) {
            return Err(format!(
                "duplicate host environment variable `{name}` on lines {first} and {number}"
            ));
        }

        entries.push((name.to_owned(), value.to_owned()));
    }

    Ok(entries)
}

// Returns the file content, or `None` when the file is absent. Any other
// read failure is a startup error.
fn read_host_environment_file() -> Result<Option<String>, String> {
    let path = host_environment_file_path();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot read host environment file {}: {error}",
                path.display()
            ));
        }
    };

    String::from_utf8(bytes).map(Some).map_err(|_| {
        format!(
            "host environment file {} is not valid UTF-8",
            path.display()
        )
    })
}

fn host_environment_file_path() -> PathBuf {
    match env::var_os(HOST_ENVIRONMENT_FILE_VARIABLE) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(DEFAULT_HOST_ENVIRONMENT_FILE),
    }
}

// Applies validated entries only where the caller environment does not
// already provide the variable.
fn apply_host_environment(entries: Vec<(String, String)>) {
    for (name, value) in entries {
        if env::var_os(&name).is_none() {
            // SAFETY: called before any threads are spawned in main()
            unsafe { env::set_var(&name, &value) };
        }
    }
}

// Derives uid-scoped runtime paths so rootless systemd and Podman never use
// another user's bus.
fn derive_runtime_environment() {
    // SAFETY: called before any threads are spawned in main()
    let uid = unsafe { libc::getuid() };
    let runtime_dir = format!("/run/user/{uid}");
    let dbus_address = format!("unix:path={runtime_dir}/bus");

    // XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS are uid-scoped: a value inherited
    // from another user (for example /run/user/0 when launched through `runuser` as
    // root) is never valid for this process, so they are always derived from the
    // effective uid.
    // SAFETY: called before any threads are spawned in main()
    unsafe {
        env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
        env::set_var("DBUS_SESSION_BUS_ADDRESS", &dbus_address);
    }

    if let Ok(home) = env::var("HOME") {
        if !home.is_empty() && env::var_os("PNEUMA_QUADLET_DIR").is_none() {
            // SAFETY: called before any threads are spawned in main()
            unsafe {
                env::set_var(
                    "PNEUMA_QUADLET_DIR",
                    format!("{home}/.config/containers/systemd"),
                );
            }
        }
    }
}

// Requires a nonempty `HOME` or `PNEUMA_QUADLET_DIR` after derivation so the
// Quadlet directory is always resolvable.
fn require_quadlet_configuration() -> Result<(), String> {
    let home_nonempty = env::var("HOME").is_ok_and(|home| !home.is_empty());
    let quadlet_nonempty =
        env::var("PNEUMA_QUADLET_DIR").is_ok_and(|directory| !directory.is_empty());

    if home_nonempty || quadlet_nonempty {
        Ok(())
    } else {
        Err("either HOME or PNEUMA_QUADLET_DIR must be set to a nonempty value".to_owned())
    }
}

fn is_valid_variable_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ignores_comments_blank_lines_and_surrounding_whitespace() {
        let entries = parse_host_environment(
            "\n# full comment\n  \n  NAME = value  \nEMPTY=\nWITH=equals=sign#not a comment\n",
        )
        .unwrap();

        assert_eq!(
            entries,
            vec![
                ("NAME".to_owned(), "value".to_owned()),
                ("EMPTY".to_owned(), String::new()),
                ("WITH".to_owned(), "equals=sign#not a comment".to_owned()),
            ]
        );
    }

    #[test]
    fn parse_reports_the_line_of_a_missing_separator() {
        let error = parse_host_environment("FIRST=1\nSECOND\n").unwrap_err();

        assert_eq!(
            error,
            "invalid host environment entry on line 2: expected NAME=VALUE"
        );
    }

    #[test]
    fn parse_reports_empty_and_invalid_variable_names() {
        assert_eq!(
            parse_host_environment("=value\n").unwrap_err(),
            "invalid host environment entry on line 1: empty variable name"
        );
        assert_eq!(
            parse_host_environment("1NAME=value\n").unwrap_err(),
            "invalid host environment entry on line 1: invalid variable name `1NAME`"
        );
        assert_eq!(
            parse_host_environment("WITH-DASH=value\n").unwrap_err(),
            "invalid host environment entry on line 1: invalid variable name `WITH-DASH`"
        );
    }

    #[test]
    fn parse_rejects_nul_bytes_in_values() {
        assert_eq!(
            parse_host_environment("NAME=value\0suffix\n").unwrap_err(),
            "invalid host environment entry on line 1: NUL byte in value"
        );
    }

    #[test]
    fn parse_rejects_duplicates_with_both_line_numbers() {
        assert_eq!(
            parse_host_environment("NAME=first\nOTHER=x\nNAME=second\n").unwrap_err(),
            "duplicate host environment variable `NAME` on lines 1 and 3"
        );
    }
}
