use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum TestGateError {
    #[error("failed to create test gate directory: {source}")]
    CreateDirectory {
        #[source]
        source: std::io::Error,
    },
    #[error("failed to create test gate marker `{}`: {source}", path.display())]
    CreateMarker {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write test gate marker `{}`: {source}", path.display())]
    WriteMarker {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

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
