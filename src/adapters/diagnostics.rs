use std::process::Command;

use rusqlite::Connection;

use crate::adapters::database;
use crate::adapters::oci_image::pull_image;
use crate::adapters::stores::release_store;
use crate::config::{
    CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE, CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
    DEFAULT_CADDY_MANAGED_PATH, DEFAULT_CADDYFILE_PATH, DEFAULT_WORKSPACE_PATH,
    WORKSPACE_PATH_ENVIRONMENT_VARIABLE, configured_path, log_verbose,
};
use crate::domain::release::OciArtifact;

const QUADLET_GENERATOR_CANDIDATES: &[&str] = &[
    "/usr/lib/systemd/user-generators/podman-user-generator",
    "/lib/systemd/user-generators/podman-user-generator",
];

// Performs the doctor command's direct host checks and prints each result in command order.
pub fn run(connection: &Connection, verbose: bool) -> bool {
    let mut all_ok = true;

    log_verbose(verbose, "checking database connection");
    match connection.query_row("SELECT 1", [], |_| Ok(())) {
        Ok(()) => println!("✓ Database connection: OK"),
        Err(source) => {
            println!("✗ Database connection: FAILED ({source})");
            all_ok = false;
        }
    }

    log_verbose(verbose, "checking database migrations");
    match database::migration_count(connection) {
        Ok(count) => println!("✓ Database migrations: {count} applied"),
        Err(source) => {
            println!("✗ Database migrations: FAILED ({source})");
            all_ok = false;
        }
    }

    log_verbose(verbose, "checking workspace directory");
    let workspace_path =
        configured_path(WORKSPACE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_WORKSPACE_PATH);
    if workspace_path.exists() {
        println!(
            "✓ Workspace directory: {} (exists)",
            workspace_path.display()
        );
    } else {
        println!(
            "✗ Workspace directory: {} (does not exist)",
            workspace_path.display()
        );
        all_ok = false;
    }

    log_verbose(verbose, "checking Caddy managed directory");
    let caddy_managed_path = configured_path(
        CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
        DEFAULT_CADDY_MANAGED_PATH,
    );
    if caddy_managed_path.exists() {
        println!(
            "✓ Caddy managed directory: {} (exists)",
            caddy_managed_path.display()
        );
    } else {
        println!(
            "✗ Caddy managed directory: {} (does not exist)",
            caddy_managed_path.display()
        );
        all_ok = false;
    }

    log_verbose(verbose, "checking Caddyfile");
    let caddyfile_path =
        configured_path(CADDYFILE_PATH_ENVIRONMENT_VARIABLE, DEFAULT_CADDYFILE_PATH);
    if caddyfile_path.exists() {
        println!("✓ Caddyfile: {} (exists)", caddyfile_path.display());
        match Command::new("caddy")
            .args(["validate", "--config"])
            .arg(&caddyfile_path)
            .args(["--adapter", "caddyfile"])
            .output()
        {
            Ok(output) if output.status.success() => println!("✓ Caddy configuration: valid"),
            Ok(output) => {
                println!(
                    "✗ Caddy configuration: FAILED ({})",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                all_ok = false;
            }
            Err(source) => {
                println!("✗ Caddy configuration: FAILED ({source})");
                all_ok = false;
            }
        }
    } else {
        println!("✗ Caddyfile: {} (does not exist)", caddyfile_path.display());
        all_ok = false;
    }

    log_verbose(verbose, "checking Git availability");
    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("✓ Git: {version}");
        }
        Ok(_) => {
            println!("✗ Git: command failed");
            all_ok = false;
        }
        Err(source) => {
            println!("✗ Git: not found ({source})");
            all_ok = false;
        }
    }

    log_verbose(verbose, "checking Podman availability");
    match Command::new("podman").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("✓ Podman: {version}");
        }
        Ok(_) => {
            println!("✗ Podman: command failed");
            all_ok = false;
        }
        Err(source) => {
            println!("✗ Podman: not found ({source})");
            all_ok = false;
        }
    }

    match release_store::active_application_image_references(connection) {
        Ok(images) => {
            for image in images {
                if let Ok(artifact) = OciArtifact::parse(&image) {
                    match pull_image(&artifact) {
                        Ok(_) => println!("✓ Active OCI image: {image} (pullable)"),
                        Err(source) => {
                            println!("✗ Active OCI image: {image} (FAILED: {source})");
                            all_ok = false;
                        }
                    }
                } else {
                    println!("- Active local image: skipped");
                }
            }
        }
        Err(source) => {
            println!("✗ Active OCI images: FAILED ({source})");
            all_ok = false;
        }
    }

    let database_path = database::configured_path();
    for path in [&database_path, &workspace_path] {
        match Command::new("df").args(["-Pk"]).arg(path).output() {
            Ok(output) if output.status.success() => {
                let free_kib = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .nth(1)
                    .and_then(|line| line.split_whitespace().nth(3))
                    .and_then(|value| value.parse::<u64>().ok());
                if free_kib.is_some_and(|value| value >= 1024 * 1024) {
                    println!("✓ Disk space: {} (at least 1 GiB free)", path.display());
                } else {
                    println!("✗ Disk space: {} (less than 1 GiB free)", path.display());
                    all_ok = false;
                }
            }
            Ok(_) | Err(_) => {
                println!("✗ Disk space: {} (unable to inspect)", path.display());
                all_ok = false;
            }
        }
    }

    match Command::new("podman")
        .args(["info", "--format", "{{.Host.Security.Rootless}}"])
        .output()
    {
        Ok(output)
            if output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == "true" =>
        {
            println!("✓ Podman rootless: OK")
        }
        Ok(output) => {
            println!(
                "✗ Podman rootless: FAILED ({})",
                String::from_utf8_lossy(&output.stdout).trim()
            );
            all_ok = false;
        }
        Err(source) => {
            println!("✗ Podman rootless: FAILED ({source})");
            all_ok = false;
        }
    }

    log_verbose(verbose, "checking Podman Quadlet user generator");
    let quadlet_generator = QUADLET_GENERATOR_CANDIDATES
        .iter()
        .find(|path| std::path::Path::new(path).is_file());
    if let Some(generator) = quadlet_generator {
        println!("✓ Podman Quadlet user generator: {generator}");
    } else {
        println!("✗ Podman Quadlet user generator: not found (install Podman >= 4.4 or Debian 13)");
        all_ok = false;
    }

    log_verbose(verbose, "checking Caddy availability");
    match Command::new("caddy").arg("version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("✓ Caddy: {version}");
        }
        Ok(_) => {
            println!("✗ Caddy: command failed");
            all_ok = false;
        }
        Err(source) => {
            println!("✗ Caddy: not found ({source})");
            all_ok = false;
        }
    }

    if all_ok {
        println!("\nAll checks passed!");
    } else {
        println!("\nSome checks failed. Please review the output above.");
    }
    all_ok
}
