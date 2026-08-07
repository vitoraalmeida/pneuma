use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use pneuma::adapters::database;
use pneuma::use_cases::deploy_internal_revision::{
    DeployInternalRevisionError, deploy_internal_revision, deploy_internal_revision_with_progress,
};
use pneuma::use_cases::import_application::import_application;

const CHILD_CASE: &str = "PNEUMA_DEPLOY_TEST_CASE";
const DATABASE_PATH: &str = "PNEUMA_DEPLOY_TEST_DATABASE";
const APPLICATION_ID: &str = "PNEUMA_DEPLOY_TEST_APPLICATION";
const REPOSITORY_PATH: &str = "PNEUMA_DEPLOY_TEST_REPOSITORY";
const WORKSPACE_PATH: &str = "PNEUMA_DEPLOY_TEST_WORKSPACE";
const FIRST_REVISION: &str = "PNEUMA_DEPLOY_TEST_FIRST_REVISION";
const SECOND_REVISION: &str = "PNEUMA_DEPLOY_TEST_SECOND_REVISION";

#[test]
fn deploys_an_internal_revision_through_promotion() {
    let project = TestProject::new("internal");
    let (endpoint, server) = server_with_statuses(&[200]);

    project.run_child(
        "success",
        &[("PNEUMA_FAKE_FIRST_PORT", endpoint.port().to_string())],
    );
    server.join().unwrap();

    let connection = database::open(&project.database_path).unwrap();
    let persisted: (String, String, String, bool) = connection
        .query_row(
            "SELECT deployments.status, runtime_instances.role, revisions.commit_sha,
                    deployments.finished_at IS NOT NULL
             FROM deployments
             JOIN revisions ON revisions.id = deployments.revision_id
             JOIN runtime_instances
                ON runtime_instances.deployment_id = deployments.id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(persisted.0, "succeeded");
    assert_eq!(persisted.1, "current");
    assert_eq!(persisted.2, project.first_revision);
    assert!(persisted.3);
}

#[test]
fn records_a_build_failure_at_the_building_stage() {
    let project = TestProject::new("internal");

    project.run_child(
        "build-failure",
        &[("PNEUMA_FAKE_BUILD_FAILURE", "1".to_owned())],
    );

    let connection = database::open(&project.database_path).unwrap();
    let failure: (String, String, String, i64) = connection
        .query_row(
            "SELECT status, failure_code, failure_stage,
                    (SELECT COUNT(*) FROM runtime_instances)
             FROM deployments",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        failure,
        (
            "failed".to_owned(),
            "image_build_failed".to_owned(),
            "building".to_owned(),
            0,
        )
    );
}

#[test]
fn removes_an_unhealthy_candidate_and_preserves_current() {
    let mut project = TestProject::new("internal");
    project.second_revision = Some(project.commit("second revision"));
    let (first_endpoint, first_server) = server_with_statuses(&[200]);
    let (second_endpoint, second_server) = server_with_statuses(&[503; 5]);

    project.run_child(
        "unhealthy-replacement",
        &[
            ("PNEUMA_FAKE_FIRST_PORT", first_endpoint.port().to_string()),
            (
                "PNEUMA_FAKE_SECOND_PORT",
                second_endpoint.port().to_string(),
            ),
        ],
    );
    first_server.join().unwrap();
    second_server.join().unwrap();

    let connection = database::open(&project.database_path).unwrap();
    let first_state = state_for_revision(&connection, &project.first_revision);
    let second_state = state_for_revision(&connection, project.second_revision.as_deref().unwrap());
    assert_eq!(
        first_state,
        ("succeeded".to_owned(), "current".to_owned(), false)
    );
    assert_eq!(
        second_state,
        ("failed".to_owned(), "candidate".to_owned(), true)
    );
    let second_failure: (String, String) = connection
        .query_row(
            "SELECT deployments.failure_code, deployments.failure_stage
             FROM deployments
             JOIN revisions ON revisions.id = deployments.revision_id
             WHERE revisions.commit_sha = ?1",
            [project.second_revision.as_deref().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        second_failure,
        (
            "health_check_failed".to_owned(),
            "verifying_internal".to_owned(),
        )
    );
    let podman_log = fs::read_to_string(&project.podman_log).unwrap();
    assert!(podman_log.contains(&format!("container rm --force {}", "b".repeat(64))));
}

#[test]
fn reuses_a_healthy_current_runtime_for_the_same_revision() {
    let project = TestProject::new("internal");
    let (endpoint, server) = server_with_statuses(&[200, 200]);

    project.run_child(
        "reuse-current",
        &[("PNEUMA_FAKE_FIRST_PORT", endpoint.port().to_string())],
    );
    server.join().unwrap();

    let connection = database::open(&project.database_path).unwrap();
    let counts: (i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM deployments),
                (SELECT COUNT(*) FROM runtime_instances)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1));
    let podman_log = fs::read_to_string(&project.podman_log).unwrap();
    assert_eq!(
        podman_log
            .lines()
            .filter(|line| line.starts_with("create "))
            .count(),
        1
    );
}

#[test]
fn restarts_a_stopped_current_runtime_for_the_same_revision() {
    let project = TestProject::new("internal");
    let (endpoint, server) = server_with_statuses(&[200, 200]);

    project.run_child(
        "restart-current",
        &[
            ("PNEUMA_FAKE_FIRST_PORT", endpoint.port().to_string()),
            ("PNEUMA_FAKE_RECONCILE_STATE", "stopped".to_owned()),
        ],
    );
    server.join().unwrap();

    let connection = database::open(&project.database_path).unwrap();
    let persisted: (i64, String) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM deployments),
                last_observed_state
             FROM runtime_instances WHERE role = 'current'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(persisted, (1, "running".to_owned()));
    let podman_log = fs::read_to_string(&project.podman_log).unwrap();
    assert_eq!(
        podman_log
            .lines()
            .filter(|line| line.starts_with("start "))
            .count(),
        2
    );
}

#[test]
fn replaces_a_current_runtime_that_is_missing_from_podman() {
    let project = TestProject::new("internal");
    let (first_endpoint, first_server) = server_with_statuses(&[200]);
    let (second_endpoint, second_server) = server_with_statuses(&[200]);

    project.run_child(
        "replace-missing",
        &[
            ("PNEUMA_FAKE_FIRST_PORT", first_endpoint.port().to_string()),
            (
                "PNEUMA_FAKE_SECOND_PORT",
                second_endpoint.port().to_string(),
            ),
            ("PNEUMA_FAKE_MISSING_ON_RECONCILE", "1".to_owned()),
        ],
    );
    first_server.join().unwrap();
    second_server.join().unwrap();

    let connection = database::open(&project.database_path).unwrap();
    let counts: (i64, i64, i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM deployments),
                (SELECT COUNT(*) FROM runtime_instances),
                (SELECT COUNT(*) FROM runtime_instances WHERE removed_at IS NOT NULL),
                (SELECT COUNT(*) FROM runtime_instances
                 WHERE role = 'current' AND removed_at IS NULL)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(counts, (2, 2, 1, 1));
    let podman_log = fs::read_to_string(&project.podman_log).unwrap();
    assert_eq!(
        podman_log
            .lines()
            .filter(|line| line.starts_with("create "))
            .count(),
        2
    );
}

#[test]
fn preserves_an_unhealthy_current_runtime_for_manual_recovery() {
    let project = TestProject::new("internal");
    let statuses = [200, 503, 503, 503, 503, 503];
    let (endpoint, server) = server_with_statuses(&statuses);

    project.run_child(
        "unhealthy-current",
        &[("PNEUMA_FAKE_FIRST_PORT", endpoint.port().to_string())],
    );
    server.join().unwrap();

    let connection = database::open(&project.database_path).unwrap();
    let persisted: (i64, String, bool) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM deployments),
                role,
                removed_at IS NOT NULL
             FROM runtime_instances",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(persisted, (1, "current".to_owned(), false));
    let podman_log = fs::read_to_string(&project.podman_log).unwrap();
    assert_eq!(
        podman_log
            .lines()
            .filter(|line| line.starts_with("create "))
            .count(),
        1
    );
}

#[test]
fn reactivates_a_healthy_previous_runtime_without_rebuilding() {
    let mut project = TestProject::new("internal");
    project.second_revision = Some(project.commit("second revision"));
    let (first_endpoint, first_server) = server_with_statuses(&[200, 200]);
    let (second_endpoint, second_server) = server_with_statuses(&[200]);

    project.run_child(
        "reactivate-previous",
        &[
            ("PNEUMA_FAKE_FIRST_PORT", first_endpoint.port().to_string()),
            (
                "PNEUMA_FAKE_SECOND_PORT",
                second_endpoint.port().to_string(),
            ),
        ],
    );
    first_server.join().unwrap();
    second_server.join().unwrap();

    let connection = database::open(&project.database_path).unwrap();
    assert_eq!(
        state_for_revision(&connection, &project.first_revision).1,
        "current"
    );
    assert_eq!(
        state_for_revision(&connection, project.second_revision.as_deref().unwrap()).1,
        "previous"
    );
    let podman_log = fs::read_to_string(&project.podman_log).unwrap();
    assert_eq!(
        podman_log
            .lines()
            .filter(|line| line.starts_with("create "))
            .count(),
        2
    );
}

#[test]
fn restarts_a_stopped_previous_runtime_before_reactivation() {
    let mut project = TestProject::new("internal");
    project.second_revision = Some(project.commit("second revision"));
    let (first_endpoint, first_server) = server_with_statuses(&[200, 200]);
    let (second_endpoint, second_server) = server_with_statuses(&[200]);

    project.run_child(
        "reactivate-previous",
        &[
            ("PNEUMA_FAKE_FIRST_PORT", first_endpoint.port().to_string()),
            (
                "PNEUMA_FAKE_SECOND_PORT",
                second_endpoint.port().to_string(),
            ),
            ("PNEUMA_FAKE_RECONCILE_STATE", "stopped-previous".to_owned()),
        ],
    );
    first_server.join().unwrap();
    second_server.join().unwrap();

    let podman_log = fs::read_to_string(&project.podman_log).unwrap();
    assert_eq!(
        podman_log
            .lines()
            .filter(|line| line.starts_with("start "))
            .count(),
        3
    );
}

#[test]
fn preserves_roles_when_a_previous_runtime_is_unhealthy() {
    let mut project = TestProject::new("internal");
    project.second_revision = Some(project.commit("second revision"));
    let first_statuses = [200, 503, 503, 503, 503, 503];
    let (first_endpoint, first_server) = server_with_statuses(&first_statuses);
    let (second_endpoint, second_server) = server_with_statuses(&[200]);

    project.run_child(
        "unhealthy-previous",
        &[
            ("PNEUMA_FAKE_FIRST_PORT", first_endpoint.port().to_string()),
            (
                "PNEUMA_FAKE_SECOND_PORT",
                second_endpoint.port().to_string(),
            ),
        ],
    );
    first_server.join().unwrap();
    second_server.join().unwrap();

    let connection = database::open(&project.database_path).unwrap();
    assert_eq!(
        state_for_revision(&connection, &project.first_revision).1,
        "previous"
    );
    assert_eq!(
        state_for_revision(&connection, project.second_revision.as_deref().unwrap()).1,
        "current"
    );
}

#[test]
fn refuses_a_public_application_before_external_work() {
    let project = TestProject::new("public");
    let mut connection = database::open(&project.database_path).unwrap();
    let unused_workspace = project.root.join("unused-workspace");

    let error = deploy_internal_revision(
        &mut connection,
        &project.application_id,
        Path::new("missing-repository"),
        "main",
        &unused_workspace,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        DeployInternalRevisionError::PublicApplication { .. }
    ));
    let deployment_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM deployments", [], |row| row.get(0))
        .unwrap();
    assert_eq!(deployment_count, 0);
    assert!(!unused_workspace.exists());
}

#[test]
fn rejects_an_unknown_revision_without_deployment_history() {
    let project = TestProject::new("internal");
    let mut connection = database::open(&project.database_path).unwrap();
    let unused_workspace = project.root.join("unused-workspace");

    let error = deploy_internal_revision(
        &mut connection,
        &project.application_id,
        &project.repository_path,
        "missing-revision",
        &unused_workspace,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        DeployInternalRevisionError::ResolveRevision { .. }
    ));
    let deployment_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM deployments", [], |row| row.get(0))
        .unwrap();
    assert_eq!(deployment_count, 0);
    assert!(!unused_workspace.exists());
}

#[test]
fn deployment_child_process() {
    let Some(case) = env::var_os(CHILD_CASE) else {
        return;
    };
    let database_path = required_path(DATABASE_PATH);
    let application_id = required_string(APPLICATION_ID);
    let repository_path = required_path(REPOSITORY_PATH);
    let workspace_path = required_path(WORKSPACE_PATH);
    let first_revision = required_string(FIRST_REVISION);
    let mut connection = database::open(&database_path).unwrap();

    match case.to_str().unwrap() {
        "success" => {
            let deployed = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            assert_eq!(deployed.commit_sha, first_revision);
        }
        "build-failure" => {
            let error = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                DeployInternalRevisionError::DeploymentFailed {
                    code: "image_build_failed",
                    ..
                }
            ));
        }
        "unhealthy-replacement" => {
            deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            let second_revision = required_string(SECOND_REVISION);
            let error = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &second_revision,
                &workspace_path,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                DeployInternalRevisionError::DeploymentFailed {
                    code: "health_check_failed",
                    ..
                }
            ));
        }
        "reuse-current" => {
            let first = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            let second = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            assert_eq!(second, first);
        }
        "restart-current" => {
            let first = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            let second = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            assert_eq!(second, first);
        }
        "replace-missing" => {
            let first = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            let second = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            assert_ne!(second.deployment_id, first.deployment_id);
            assert_ne!(second.runtime_id, first.runtime_id);
            assert_eq!(second.commit_sha, first.commit_sha);
        }
        "unhealthy-current" => {
            deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            let error = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                DeployInternalRevisionError::ExistingRuntimeUnhealthy { .. }
            ));
        }
        "reactivate-previous" => {
            let first = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            let second_revision = required_string(SECOND_REVISION);
            let second = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &second_revision,
                &workspace_path,
            )
            .unwrap();
            let mut progress = Vec::new();
            let mut report =
                |event: pneuma::use_cases::deploy_internal_revision::DeploymentProgress| {
                    progress.push(event.to_string());
                };
            let reactivated = deploy_internal_revision_with_progress(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
                &mut report,
            )
            .unwrap();
            assert_eq!(reactivated, first);
            assert_ne!(reactivated.runtime_id, second.runtime_id);
            assert!(
                progress
                    .iter()
                    .any(|message| message.contains("reactivated as Current"))
            );
        }
        "unhealthy-previous" => {
            deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap();
            let second_revision = required_string(SECOND_REVISION);
            deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &second_revision,
                &workspace_path,
            )
            .unwrap();
            let error = deploy_internal_revision(
                &mut connection,
                &application_id,
                &repository_path,
                &first_revision,
                &workspace_path,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                DeployInternalRevisionError::ExistingRuntimeUnhealthy { .. }
            ));
        }
        unknown => panic!("unknown child case: {unknown}"),
    }
}

fn state_for_revision(
    connection: &rusqlite::Connection,
    commit_sha: &str,
) -> (String, String, bool) {
    connection
        .query_row(
            "SELECT deployments.status, runtime_instances.role,
                    runtime_instances.removed_at IS NOT NULL
             FROM deployments
             JOIN revisions ON revisions.id = deployments.revision_id
             JOIN runtime_instances
                ON runtime_instances.deployment_id = deployments.id
             WHERE revisions.commit_sha = ?1",
            [commit_sha],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

struct TestProject {
    root: PathBuf,
    repository_path: PathBuf,
    database_path: PathBuf,
    workspace_path: PathBuf,
    fake_bin: PathBuf,
    podman_log: PathBuf,
    application_id: String,
    first_revision: String,
    second_revision: Option<String>,
}

impl TestProject {
    fn new(visibility: &str) -> Self {
        let root = env::temp_dir().join(format!(
            "pneuma-deploy-revision-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let repository_path = root.join("repository");
        let database_path = root.join("pneuma.sqlite3");
        let workspace_path = root.join("workspaces");
        let fake_bin = root.join("bin");
        let podman_log = root.join("podman.log");
        fs::create_dir_all(&repository_path).unwrap();
        fs::create_dir(&fake_bin).unwrap();

        let manifest = if visibility == "public" {
            manifest("public", "domain = \"example.com\"\n")
        } else {
            manifest("internal", "")
        };
        fs::write(repository_path.join("pneuma.toml"), manifest).unwrap();
        fs::write(repository_path.join("Containerfile"), "FROM scratch\n").unwrap();
        fs::write(repository_path.join("site.txt"), "first revision\n").unwrap();
        initialize_repository(&repository_path);
        install_fake_podman(&fake_bin);

        let first_revision = git(&repository_path, &["rev-parse", "HEAD"])
            .trim()
            .to_owned();
        let mut connection = database::open(&database_path).unwrap();
        let application = import_application(&mut connection, &repository_path).unwrap();

        Self {
            root,
            repository_path,
            database_path,
            workspace_path,
            fake_bin,
            podman_log,
            application_id: application.id,
            first_revision,
            second_revision: None,
        }
    }

    fn commit(&self, contents: &str) -> String {
        fs::write(self.repository_path.join("site.txt"), contents).unwrap();
        git(&self.repository_path, &["add", "site.txt"]);
        git(
            &self.repository_path,
            &[
                "-c",
                "user.name=Pneuma Tests",
                "-c",
                "user.email=pneuma@example.invalid",
                "commit",
                "--quiet",
                "-m",
                contents,
            ],
        );
        git(&self.repository_path, &["rev-parse", "HEAD"])
            .trim()
            .to_owned()
    }

    fn run_child(&self, case: &str, extra_environment: &[(&str, String)]) {
        let path = executable_path(&self.fake_bin);
        let count_path = self.root.join("podman-count");
        let mut command = Command::new(env::current_exe().unwrap());
        command
            .args(["--exact", "deployment_child_process", "--nocapture"])
            .env(CHILD_CASE, case)
            .env(DATABASE_PATH, &self.database_path)
            .env(APPLICATION_ID, &self.application_id)
            .env(REPOSITORY_PATH, &self.repository_path)
            .env(WORKSPACE_PATH, &self.workspace_path)
            .env(FIRST_REVISION, &self.first_revision)
            .env("PATH", path)
            .env("PNEUMA_FAKE_PODMAN_LOG", &self.podman_log)
            .env("PNEUMA_FAKE_PODMAN_COUNT", count_path)
            .env(
                "PNEUMA_FAKE_PODMAN_OBSERVE_COUNT",
                self.root.join("podman-observe-count"),
            );
        if let Some(second_revision) = &self.second_revision {
            command.env(SECOND_REVISION, second_revision);
        }
        for (name, value) in extra_environment {
            command.env(name, value);
        }

        let output = command.output().unwrap();
        assert_command_succeeded(&output);
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn manifest(visibility: &str, domain: &str) -> String {
    format!(
        "schema_version = 1\n\
         \n\
         [application]\n\
         name = \"deploy-test\"\n\
         \n\
         [source]\n\
         repository = \".\"\n\
         branch = \"main\"\n\
         \n\
         [build]\n\
         containerfile = \"Containerfile\"\n\
         context = \".\"\n\
         \n\
         [runtime]\n\
         container_port = 8080\n\
         healthcheck_path = \"/healthz\"\n\
         expected_status = 200\n\
         \n\
         [exposure]\n\
         default_visibility = \"{visibility}\"\n\
         {domain}"
    )
}

fn initialize_repository(repository_path: &Path) {
    let output = Command::new("git")
        .args(["init", "--quiet", "--initial-branch=main"])
        .arg(repository_path)
        .output()
        .unwrap();
    assert_command_succeeded(&output);
    git(repository_path, &["add", "."]);
    git(
        repository_path,
        &[
            "-c",
            "user.name=Pneuma Tests",
            "-c",
            "user.email=pneuma@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "first revision",
        ],
    );
}

fn git(repository_path: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args(arguments)
        .output()
        .unwrap();
    assert_command_succeeded(&output);
    String::from_utf8(output.stdout).unwrap()
}

fn install_fake_podman(fake_bin: &Path) {
    let podman = fake_bin.join("podman");
    fs::write(
        &podman,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$PNEUMA_FAKE_PODMAN_LOG"

case "$1" in
    build)
        if [ "${PNEUMA_FAKE_BUILD_FAILURE:-0}" = "1" ]; then
            printf 'invalid Containerfile\n' >&2
            exit 1
        fi
        ;;
    create)
        count=0
        if [ -f "$PNEUMA_FAKE_PODMAN_COUNT" ]; then
            count=$(sed -n '1p' "$PNEUMA_FAKE_PODMAN_COUNT")
        fi
        count=$((count + 1))
        printf '%s\n' "$count" > "$PNEUMA_FAKE_PODMAN_COUNT"
        if [ "$count" -eq 1 ]; then
            printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'
        else
            printf 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n'
        fi
        ;;
    start)
        ;;
    container)
        if [ "${2:-}" = "exists" ]; then
            count=0
            if [ -f "$PNEUMA_FAKE_PODMAN_OBSERVE_COUNT" ]; then
                count=$(sed -n '1p' "$PNEUMA_FAKE_PODMAN_OBSERVE_COUNT")
            fi
            count=$((count + 1))
            printf '%s\n' "$count" > "$PNEUMA_FAKE_PODMAN_OBSERVE_COUNT"
            if [ "${PNEUMA_FAKE_MISSING_ON_RECONCILE:-0}" = "1" ] && [ "$count" -eq 2 ]; then
                exit 1
            fi
        fi
        ;;
    inspect)
        count=$(sed -n '1p' "$PNEUMA_FAKE_PODMAN_OBSERVE_COUNT")
        reconcile_state="${PNEUMA_FAKE_RECONCILE_STATE:-}"
        if { [ "$reconcile_state" = "stopped" ] && [ "$count" -eq 2 ]; } ||
           { [ "$reconcile_state" = "stopped-previous" ] && [ "$count" -eq 3 ]; }; then
            printf 'stopped\n'
        else
            printf 'running\n'
        fi
        ;;
    port)
        case "$2" in
            a*) printf '127.0.0.1:%s\n' "$PNEUMA_FAKE_FIRST_PORT" ;;
            b*) printf '127.0.0.1:%s\n' "$PNEUMA_FAKE_SECOND_PORT" ;;
        esac
        ;;
    *)
        printf 'unsupported fake Podman command: %s\n' "$*" >&2
        exit 1
        ;;
esac
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&podman).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(podman, permissions).unwrap();
}

fn executable_path(fake_bin: &Path) -> OsString {
    let inherited = env::var_os("PATH").unwrap_or_default();
    env::join_paths(std::iter::once(fake_bin.to_path_buf()).chain(env::split_paths(&inherited)))
        .unwrap()
}

fn server_with_statuses(statuses: &[u16]) -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let endpoint = listener.local_addr().unwrap();
    let statuses = statuses.to_vec();
    let server = thread::spawn(move || {
        for status in statuses {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n");
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (endpoint, server)
}

fn read_request(stream: &mut TcpStream) {
    let mut request = Vec::new();
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let mut buffer = [0; 1024];
        let bytes_read = stream.read(&mut buffer).unwrap();
        assert_ne!(bytes_read, 0);
        request.extend_from_slice(&buffer[..bytes_read]);
    }
}

fn required_path(name: &str) -> PathBuf {
    PathBuf::from(env::var_os(name).unwrap())
}

fn required_string(name: &str) -> String {
    env::var(name).unwrap()
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn assert_command_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
