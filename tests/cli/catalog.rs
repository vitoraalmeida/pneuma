use std::ffi::OsStr;
use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::thread;

use pneuma::adapters::database;

use crate::support::{
    DeploymentEnvironment, assert_command_succeeded, create_repository_from_fixture, fixture_path,
    initialize_repository, respond_once, run_pneuma, run_pneuma_env, temporary_database_path,
    temporary_workspace_path,
};

#[test]
fn imports_and_lists_an_application_idempotently() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = create_repository_from_fixture(&workspace, "valid");
    let url = format!("file://{}", repository_path.display());

    let first_import = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );
    let second_import = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );
    let list = run_pneuma(&database_path, &[OsStr::new("app"), OsStr::new("list")]);
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);

    assert!(first_import.status.success());
    assert!(second_import.status.success());
    assert_eq!(
        String::from_utf8_lossy(&first_import.stdout),
        "Imported personal-site\nStatus: Registered\nDeployment: Not deployed\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&list.stdout),
        "personal-site\tRegistered\tNot deployed\n"
    );
}

#[test]
fn reports_manifest_errors_and_returns_failure() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = workspace.join("remote");
    fs::create_dir_all(&repository_path).unwrap();
    fs::write(repository_path.join("README.md"), "missing manifest\n").unwrap();
    initialize_repository(&repository_path);
    let url = format!("file://{}", repository_path.display());

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("pneuma.toml"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_a_local_import_path_without_creating_an_application() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = fixture_path("valid");

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            repository_path.as_os_str(),
        ],
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("application imports require a Git URL; local paths are not supported")
    );
    let connection = database::open(&database_path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert!(!workspace.exists());
    let _ = fs::remove_file(&database_path);
}

#[test]
fn cleans_the_temporary_checkout_after_a_clone_failure() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let url = format!("file://{}/missing", workspace.display());

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );

    assert!(!output.status.success());
    let imports = workspace.join("imports");
    assert!(imports.exists());
    assert!(fs::read_dir(&imports).unwrap().next().is_none());
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);
}

#[test]
fn imports_from_a_remote_git_url_with_a_manifest_path() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let remote = workspace.join("remote");
    fs::create_dir_all(remote.join("deploy/staging")).unwrap();
    fs::copy(
        fixture_path("valid/deploy/staging/pneuma.toml"),
        remote.join("deploy/staging/pneuma.toml"),
    )
    .unwrap();
    initialize_repository(&remote);
    let url = format!("file://{}", remote.display());

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[
            OsStr::new("app"),
            OsStr::new("import"),
            OsStr::new(&url),
            OsStr::new("--manifest"),
            OsStr::new("deploy/staging/pneuma.toml"),
        ],
    );

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Imported personal-site\nStatus: Registered\nDeployment: Not deployed\n"
    );
    let connection = database::open(&database_path).unwrap();
    let (repository_url, manifest_path): (String, String) = connection
        .query_row(
            "SELECT repository_url, manifest_path FROM applications",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(repository_url, url);
    assert_eq!(manifest_path, "deploy/staging/pneuma.toml");
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);
}

#[test]
fn import_without_a_required_system_fails_with_usage_exit_code() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let repository_path = workspace.join("remote");
    fs::create_dir_all(&repository_path).unwrap();
    fs::write(
        repository_path.join("pneuma.toml"),
        concat!(
            "schema_version = 3\n",
            "\n",
            "[application]\n",
            "name = \"systemless-site\"\n",
            "\n",
            "[delivery]\n",
            "type = \"oci\"\n",
            "image = \"registry.example/team/service\"\n",
            "\n",
            "[runtime]\n",
            "container_port = 8080\n",
            "healthcheck_path = \"/healthz\"\n",
            "expected_status = 200\n",
            "\n",
            "[exposure]\n",
            "default_visibility = \"internal\"\n",
        ),
    )
    .unwrap();
    initialize_repository(&repository_path);
    let url = format!("file://{}", repository_path.display());

    let output = run_pneuma_env(
        &database_path,
        Some(&workspace),
        &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)],
    );
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("system is required"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn remote_import_is_idempotent() {
    let database_path = temporary_database_path();
    let workspace = temporary_workspace_path();
    let remote = workspace.join("remote");
    fs::create_dir_all(&remote).unwrap();
    fs::copy(
        fixture_path("valid/pneuma.toml"),
        remote.join("pneuma.toml"),
    )
    .unwrap();
    initialize_repository(&remote);
    let url = format!("file://{}", remote.display());
    let arguments = &[OsStr::new("app"), OsStr::new("import"), OsStr::new(&url)];

    let first = run_pneuma_env(&database_path, Some(&workspace), arguments);
    let second = run_pneuma_env(&database_path, Some(&workspace), arguments);

    assert!(first.status.success());
    assert!(second.status.success());
    let connection = database::open(&database_path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let exposure_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM exposures", [], |row| row.get(0))
        .unwrap();
    assert_eq!(exposure_count, 1);
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_file(&database_path);
}

#[test]
fn reimport_reports_the_real_state_of_a_deployed_application() {
    let environment = DeploymentEnvironment::new();
    assert_command_succeeded(&environment.import());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || respond_once(&listener, 200));
    let deploy = environment.deploy(port, false);
    server.join().unwrap();
    assert_command_succeeded(&deploy);
    let deployment_id = String::from_utf8(deploy.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Deployment: "))
        .unwrap()
        .to_owned();

    let reimport = environment.import();
    assert_command_succeeded(&reimport);
    let stdout = String::from_utf8(reimport.stdout).unwrap();
    assert!(
        stdout.contains(&format!("Deployment: {deployment_id}")),
        "unexpected stdout: {stdout}"
    );
    assert!(
        !stdout.contains("Not deployed"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn systems_round_trip_through_the_cli_with_unchanged_output() {
    let database_path = temporary_database_path();

    let create = run_pneuma(
        &database_path,
        &[
            OsStr::new("system"),
            OsStr::new("create"),
            OsStr::new("platform"),
            OsStr::new("--description"),
            OsStr::new("Team platform"),
        ],
    );
    let list = run_pneuma(&database_path, &[OsStr::new("system"), OsStr::new("list")]);
    let show = run_pneuma(
        &database_path,
        &[
            OsStr::new("system"),
            OsStr::new("show"),
            OsStr::new("platform"),
        ],
    );
    let missing = run_pneuma(
        &database_path,
        &[
            OsStr::new("system"),
            OsStr::new("show"),
            OsStr::new("missing"),
        ],
    );
    let invalid = run_pneuma(
        &database_path,
        &[
            OsStr::new("system"),
            OsStr::new("create"),
            OsStr::new("Not Valid"),
        ],
    );
    let _ = fs::remove_file(&database_path);

    assert_command_succeeded(&create);
    assert_eq!(
        String::from_utf8_lossy(&create.stdout),
        "Created platform\n"
    );
    assert_command_succeeded(&list);
    assert_eq!(String::from_utf8_lossy(&list.stdout), "platform\n");
    assert_command_succeeded(&show);
    assert_eq!(
        String::from_utf8_lossy(&show.stdout),
        "System: platform\nDescription: Team platform\nApplications: (none)\n"
    );
    assert_eq!(missing.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(
        stderr.contains("error: system `missing` was not found"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(invalid.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(
        stderr.contains("error: invalid system name `Not Valid`"),
        "unexpected stderr: {stderr}"
    );
}
