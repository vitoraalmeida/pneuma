//! Process-scoped test support for adapter tests that fake external commands.
//!
//! Adapter tests exercise real `Command` execution by placing fake executables
//! on `PATH`. Environment variables are process-global, so every override is
//! serialized behind one mutex shared by the whole test binary.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static EXTERNAL_PATH_LOCK: Mutex<()> = Mutex::new(());

// Installs a directory of fake executables as the only PATH entry for the
// duration of the guard, so adapter tests never reach real external tools.
pub(crate) struct ScopedExternalPath {
    _guard: MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
    directory: PathBuf,
}

impl ScopedExternalPath {
    pub(crate) fn new(name: &str, scripts: &[(&str, &str)]) -> Self {
        let guard = EXTERNAL_PATH_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let directory = std::env::temp_dir().join(format!(
            "pneuma-adapter-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        for (file_name, contents) in scripts {
            let path = directory.join(file_name);
            std::fs::write(&path, contents).unwrap();
            make_executable(&path);
        }
        let previous = std::env::var_os("PATH");
        // Safety: all PATH reads and writes in test processes happen while
        // holding EXTERNAL_PATH_LOCK, which `guard` keeps alive.
        unsafe { std::env::set_var("PATH", &directory) };
        Self {
            _guard: guard,
            previous,
            directory,
        }
    }

    pub(crate) fn directory(&self) -> &std::path::Path {
        &self.directory
    }

    pub(crate) fn set_var(&self, name: &str, value: &str) {
        // Safety: see ScopedExternalPath::new.
        unsafe { std::env::set_var(name, value) };
    }

    pub(crate) fn remove_var(&self, name: &str) {
        // Safety: see ScopedExternalPath::new.
        unsafe { std::env::remove_var(name) };
    }
}

impl Drop for ScopedExternalPath {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => {
                // Safety: see ScopedExternalPath::new.
                unsafe { std::env::set_var("PATH", previous) };
            }
            None => {
                // Safety: see ScopedExternalPath::new.
                unsafe { std::env::remove_var("PATH") };
            }
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}
