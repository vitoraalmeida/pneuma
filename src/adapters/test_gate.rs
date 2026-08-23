use std::env;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[derive(Debug)]
pub(crate) enum TestGateError {
    CreateDirectory {
        source: std::io::Error,
    },
    CreateMarker {
        path: PathBuf,
        source: std::io::Error,
    },
    WriteMarker {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for TestGateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDirectory { source } => {
                write!(formatter, "failed to create test gate directory: {source}")
            }
            Self::CreateMarker { path, source } => {
                write!(
                    formatter,
                    "failed to create test gate marker `{}`: {source}",
                    path.display()
                )
            }
            Self::WriteMarker { path, source } => {
                write!(
                    formatter,
                    "failed to write test gate marker `{}`: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for TestGateError {}

// Blocks only when the VM harness explicitly supplies a gate directory.
pub(crate) fn wait_for_test_gate(name: &str) -> Result<(), TestGateError> {
    let Some(directory) = env::var_os("PNEUMA_TEST_GATE_DIRECTORY") else {
        return Ok(());
    };
    let directory = PathBuf::from(directory);
    fs::create_dir_all(&directory).map_err(|source| TestGateError::CreateDirectory { source })?;
    let marker = directory.join(format!("{name}.ready"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .map_err(|source| TestGateError::CreateMarker {
            path: marker.clone(),
            source,
        })?;
    file.write_all(b"waiting\n")
        .map_err(|source| TestGateError::WriteMarker {
            path: marker,
            source,
        })?;
    let release = directory.join(format!("{name}.release"));
    while !release.exists() {
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
