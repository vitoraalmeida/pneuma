use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temporary_database_path() -> PathBuf {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pneuma-cli-invocation-{}-{unique_suffix}.sqlite3",
        std::process::id()
    ))
}

fn assert_database_was_not_created(database_path: &PathBuf) {
    assert!(
        !database_path.exists(),
        "the invocation must not create the database at {}",
        database_path.display()
    );
    let _ = fs::remove_file(database_path);
}

#[test]
fn reports_usage_for_an_unknown_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .args(["unknown"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized subcommand"));
    assert!(stderr.contains("Usage"));
}

#[test]
fn direct_version_prints_the_exact_release_line_without_touching_the_database() {
    let database_path = temporary_database_path();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["version"])
        .output()
        .unwrap();
    assert_database_was_not_created(&database_path);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("pneuma {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn ci_dispatched_version_prints_the_exact_release_line_without_touching_the_database() {
    let database_path = temporary_database_path();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .env("SSH_ORIGINAL_COMMAND", "version")
        .args(["ci", "dispatch"])
        .output()
        .unwrap();
    assert_database_was_not_created(&database_path);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("pneuma {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn ci_dispatch_fails_without_ssh_original_command_and_creates_no_database() {
    let database_path = temporary_database_path();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["ci", "dispatch"])
        .output()
        .unwrap();
    assert_database_was_not_created(&database_path);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: SSH_ORIGINAL_COMMAND not set\n"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn tui_rejects_non_interactive_streams_without_creating_the_database() {
    let database_path = temporary_database_path();

    let output = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["tui"])
        .output()
        .unwrap();
    assert_database_was_not_created(&database_path);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: the terminal interface requires interactive stdin and stdout\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn tui_quit_restores_the_pseudo_terminal_mode() {
    use std::fs::File;
    use std::io::Write;
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::thread;

    fn duplicate(file: &File) -> File {
        let descriptor = unsafe { libc::dup(file.as_raw_fd()) };
        assert_ne!(descriptor, -1, "failed to duplicate pseudo-terminal");
        unsafe { File::from_raw_fd(descriptor) }
    }

    fn local_flags(file: &File) -> libc::tcflag_t {
        let mut attributes = MaybeUninit::<libc::termios>::uninit();
        let result = unsafe { libc::tcgetattr(file.as_raw_fd(), attributes.as_mut_ptr()) };
        assert_eq!(result, 0, "failed to inspect pseudo-terminal mode");
        unsafe { attributes.assume_init().c_lflag }
    }

    let mut master = -1;
    let mut slave = -1;
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "failed to open pseudo-terminal");

    let mut input = unsafe { File::from_raw_fd(master) };
    let terminal = unsafe { File::from_raw_fd(slave) };
    let original_flags = local_flags(&terminal);
    let database_path = temporary_database_path();

    let mut child = Command::new(env!("CARGO_BIN_EXE_pneuma"))
        .env("PNEUMA_DATABASE_PATH", &database_path)
        .args(["tui"])
        .stdin(Stdio::from(duplicate(&terminal)))
        .stdout(Stdio::from(duplicate(&terminal)))
        .stderr(Stdio::from(duplicate(&terminal)))
        .spawn()
        .unwrap();

    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while local_flags(&terminal) == original_flags {
        assert!(
            Instant::now() < ready_deadline,
            "TUI did not enter raw mode"
        );
        thread::sleep(Duration::from_millis(10));
    }
    input.write_all(b"q").unwrap();
    input.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "TUI did not exit after q");
        thread::sleep(Duration::from_millis(10));
    };

    assert!(status.success(), "TUI did not exit successfully: {status}");
    assert_eq!(local_flags(&terminal), original_flags);
}
