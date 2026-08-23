use std::env;
use std::fs;
use std::process::ExitCode;

mod cli;

const HOST_ENVIRONMENT_FILE: &str = "/etc/pneuma/environment";

// Loads host defaults without overriding explicit environment supplied by the caller.
fn load_host_environment() {
    let content = match fs::read_to_string(HOST_ENVIRONMENT_FILE) {
        Ok(c) => c,
        Err(_) => return,
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if !key.is_empty() && env::var_os(key).is_none() {
                // SAFETY: called before any threads are spawned in main()
                unsafe { env::set_var(key, value) };
            }
        }
    }
}

// Derives uid-scoped runtime paths so rootless systemd and Podman never use another user's bus.
fn configure_runtime_environment() {
    let uid = unsafe { libc::getuid() };
    let runtime_dir = format!("/run/user/{}", uid);
    let dbus_address = format!("unix:path={}/bus", runtime_dir);

    // XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS are uid-scoped: a value inherited
    // from another user (for example /run/user/0 when launched through `runuser` as
    // root) is never valid for this process, so they are always derived from the
    // effective uid.
    unsafe {
        env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
        env::set_var("DBUS_SESSION_BUS_ADDRESS", &dbus_address);
    }

    if let Ok(home) = env::var("HOME") {
        let quadlet_dir = format!("{}/.config/containers/systemd", home);
        if env::var_os("PNEUMA_QUADLET_DIR").is_none() {
            unsafe {
                env::set_var("PNEUMA_QUADLET_DIR", &quadlet_dir);
            }
        }
    }
}

// Initializes process-wide environment before parsing and dispatching the CLI request.
fn main() -> ExitCode {
    load_host_environment();
    configure_runtime_environment();

    let result = cli::run(cli::parse_invocation());

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
