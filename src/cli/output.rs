use std::fmt::Write as _;
use std::path::Path;

use pneuma::adapters::diagnostics::{CheckOutcome, DoctorCheck, DoctorReport};
use pneuma::domain::application::{ApplicationName, ApplicationSummary, DesiredRuntimeState};
use pneuma::domain::deployment::{
    DeploymentHistory, DeploymentLifecycle, DeploymentStatus, DeploymentType,
};
use pneuma::domain::exposure::Visibility;
use pneuma::domain::git::CommitSha;
use pneuma::domain::runtime::ObservedRuntimeState;
use pneuma::domain::system::System;
use pneuma::use_cases::application::{ApplicationCatalogEntry, RuntimeObservation};
use pneuma::use_cases::deployment::DeploymentResult;
use pneuma::use_cases::exposure::ExposureChange;
use pneuma::use_cases::reconciliation::ReconciliationResult;
use pneuma::use_cases::system::SystemDetails;

// Renders command results as presentation strings so handlers stay orchestration-only.

pub(crate) fn desired_runtime_state_label(state: DesiredRuntimeState) -> &'static str {
    match state {
        DesiredRuntimeState::Running => "Running",
        DesiredRuntimeState::Stopped => "Stopped",
    }
}

pub(crate) fn observed_runtime_state_label(state: &ObservedRuntimeState) -> String {
    match state {
        ObservedRuntimeState::Missing => "Missing".to_owned(),
        ObservedRuntimeState::Created => "Created".to_owned(),
        ObservedRuntimeState::Starting => "Starting".to_owned(),
        ObservedRuntimeState::Running => "Running".to_owned(),
        ObservedRuntimeState::Stopping => "Stopping".to_owned(),
        ObservedRuntimeState::Stopped => "Stopped".to_owned(),
        ObservedRuntimeState::Failed => "Failed".to_owned(),
        ObservedRuntimeState::Unknown { status } => format!("Unknown {{ status: {status:?} }}"),
    }
}

pub(crate) fn deployment_type_label(deployment_type: DeploymentType) -> &'static str {
    match deployment_type {
        DeploymentType::Deploy => "Deploy",
        DeploymentType::Rollback => "Rollback",
    }
}

pub(crate) fn deployment_status_label(status: DeploymentStatus) -> &'static str {
    match status {
        DeploymentStatus::Pending => "Pending",
        DeploymentStatus::Starting => "Starting",
        DeploymentStatus::Verifying => "Verifying",
        DeploymentStatus::Activating => "Activating",
        DeploymentStatus::Succeeded => "Succeeded",
        DeploymentStatus::Failed => "Failed",
    }
}

pub(crate) fn visibility_label(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "Public",
        Visibility::Internal => "Internal",
    }
}

pub(crate) fn imported_application(application: &ApplicationSummary) -> String {
    let mut output = format!("Imported {}\nStatus: Registered", application.name);
    if let Some(deployment_id) = &application.active_deployment_id {
        let _ = write!(output, "\nDeployment: {deployment_id}");
    } else {
        output.push_str("\nDeployment: Not deployed");
    }
    output
}

pub(crate) fn application_list(entries: &[ApplicationCatalogEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            let deployment_status = if entry.deployed {
                "Deployed"
            } else {
                "Not deployed"
            };
            format!("{}\tRegistered\t{deployment_status}", entry.summary.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn runtime_status(observation: &RuntimeObservation) -> String {
    format!(
        "Desired state: {}\nObserved state: {}\nRuntime: {}\nContainer: {}",
        desired_runtime_state_label(observation.desired_runtime_state),
        observed_runtime_state_label(&observation.observed_runtime_state),
        observation.runtime_id,
        observation.container_id
    )
}

pub(crate) fn application_status(
    application_name: &ApplicationName,
    observation: &RuntimeObservation,
) -> String {
    format!(
        "Application: {application_name}\n{}",
        runtime_status(observation)
    )
}

pub(crate) fn application_stopped(
    application_name: &ApplicationName,
    observation: &RuntimeObservation,
) -> String {
    format!(
        "Stopped {application_name}\n{}",
        lifecycle_outcome(observation)
    )
}

pub(crate) fn application_started(
    application_name: &ApplicationName,
    observation: &RuntimeObservation,
) -> String {
    format!(
        "Started {application_name}\n{}",
        lifecycle_outcome(observation)
    )
}

pub(crate) fn lifecycle_outcome(observation: &RuntimeObservation) -> String {
    format!(
        "Desired state: {}\nObserved state: {}",
        desired_runtime_state_label(observation.desired_runtime_state),
        observed_runtime_state_label(&observation.observed_runtime_state)
    )
}

pub(crate) fn deployment_history(
    application_name: &ApplicationName,
    deployments: &[DeploymentHistory],
) -> String {
    if deployments.is_empty() {
        return format!("No deployments for {application_name}");
    }

    let mut output = format!("Deployments for {application_name}:\n");
    output.push_str(
        "DEPLOYMENT\tTYPE\tRELEASE\tSOURCE\tSTATUS\tSTARTED\tFINISHED\tACTIVE\tFAILURE\n",
    );
    for deployment in deployments {
        let source = deployment
            .deployment
            .source_revision
            .as_ref()
            .map_or("-", CommitSha::as_str);
        let (finished_at, failure) = match &deployment.deployment.lifecycle {
            DeploymentLifecycle::Succeeded { finished_at } => {
                (finished_at.as_str(), "-".to_owned())
            }
            DeploymentLifecycle::Failed { failure } => (
                failure.finished_at.as_str(),
                format!("{}:{}:{}", failure.code, failure.stage, failure.message),
            ),
            DeploymentLifecycle::Pending
            | DeploymentLifecycle::Starting
            | DeploymentLifecycle::Verifying
            | DeploymentLifecycle::Activating => ("-", "-".to_owned()),
        };
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            deployment.deployment.id,
            deployment_type_label(deployment.deployment.deployment_type),
            deployment.release.artifact.digest(),
            source,
            deployment_status_label(deployment.deployment.status()),
            deployment.deployment.started_at.as_deref().unwrap_or("-"),
            finished_at,
            if deployment.is_active { "yes" } else { "no" },
            failure,
        );
    }
    output.pop();
    output
}

pub(crate) fn deployed(
    application_name: &pneuma::domain::application::ApplicationName,
    deployed: &DeploymentResult,
) -> String {
    let mut output = format!(
        "Deployed {application_name}\nImage: {}",
        deployed.artifact.reference()
    );
    if let Some(source_revision) = &deployed.source_revision {
        let _ = write!(output, "\nSource revision: {source_revision}");
    }
    let _ = write!(
        output,
        "\nDeployment: {}\nRuntime: {}\nContainer: {}\nStatus: Succeeded",
        deployed.deployment_id, deployed.runtime_id, deployed.container_name
    );
    output
}

pub(crate) fn rollback_result(
    application_name: &pneuma::domain::application::ApplicationName,
    rolled_back: &DeploymentResult,
) -> String {
    let mut output = format!(
        "Rolled back {application_name}\nImage: {}",
        rolled_back.artifact.reference()
    );
    if let Some(source_revision) = &rolled_back.source_revision {
        let _ = write!(output, "\nSource revision: {source_revision}");
    }
    let _ = write!(
        output,
        "\nDeployment: {}\nRuntime: {}\nStatus: Succeeded",
        rolled_back.deployment_id, rolled_back.runtime_id
    );
    output
}

pub(crate) fn visibility_change(
    application_name: &pneuma::domain::application::ApplicationName,
    change: &ExposureChange,
) -> String {
    let mut output = format!(
        "Visibility for {application_name}: {}",
        visibility_label(change.visibility)
    );
    if change.visibility == Visibility::Public {
        if let Some(domain) = &change.domain {
            let _ = write!(output, "\nDomain: {domain}");
        }
    }
    output
}

pub(crate) fn reconciliation_result(
    application_name: &pneuma::domain::application::ApplicationName,
    result: &ReconciliationResult,
) -> String {
    let mut output = format!("Application: {application_name}");
    match result {
        ReconciliationResult::NoOp => output.push_str("\nResult: no-op"),
        ReconciliationResult::Deferred {
            blocking_deployment,
        } => {
            output.push_str("\nResult: deferred");
            if let Some(blocking_deployment) = blocking_deployment {
                let _ = write!(
                    output,
                    "\nBlocking deployment: {} ({})",
                    blocking_deployment.id,
                    blocking_deployment.status()
                );
            }
        }
        ReconciliationResult::Repaired {
            runtime_id,
            container_id,
        } => {
            let _ = write!(
                output,
                "\nResult: repaired\nRuntime: {runtime_id}\nContainer: {container_id}"
            );
        }
        ReconciliationResult::ManualIntervention { reason } => {
            let _ = write!(
                output,
                "\nResult: manual-intervention\nDiagnostic: {reason}"
            );
        }
        ReconciliationResult::ExposureRepaired => output.push_str("\nResult: repaired"),
        ReconciliationResult::Failed { reason } => {
            let _ = write!(output, "\nResult: failed\nDiagnostic: {reason}");
        }
        ReconciliationResult::Diverged { reason } => {
            let _ = write!(output, "\nResult: diverged\nDiagnostic: {reason}");
        }
    }
    output
}

pub(crate) fn created_system(system: &System) -> String {
    format!("Created {}", system.name)
}

pub(crate) fn system_list(systems: &[System]) -> String {
    systems
        .iter()
        .map(|system| system.name.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn system_details(details: &SystemDetails) -> String {
    let mut output = format!("System: {}", details.system.name);
    if let Some(description) = &details.system.description {
        let _ = write!(output, "\nDescription: {description}");
    }
    if details.applications.is_empty() {
        output.push_str("\nApplications: (none)");
    } else {
        output.push_str("\nApplications:");
        for application in &details.applications {
            let _ = write!(output, "\n  {}", application.name);
        }
    }
    output
}

pub(crate) fn doctor_report(report: &DoctorReport) -> String {
    let mut lines: Vec<String> = report.checks.iter().map(render_doctor_check).collect();
    lines.push(String::new());
    lines.push(if report.is_healthy() {
        "All checks passed!".to_owned()
    } else {
        "Some checks failed. Please review the output above.".to_owned()
    });
    lines.join("\n")
}

pub(crate) fn database_backup(path: &Path) -> String {
    format!("Database backup: {}", path.display())
}

pub(crate) fn database_restore(path: &Path, pre_restore_path: &Path) -> String {
    format!(
        "Database restored from {}\nPre-restore backup: {}",
        path.display(),
        pre_restore_path.display()
    )
}

fn render_doctor_check(check: &DoctorCheck) -> String {
    match check {
        DoctorCheck::DatabaseConnection(outcome) => {
            format_doctor_outcome("Database connection", outcome, "OK")
        }
        DoctorCheck::DatabaseSchema(outcome) => match outcome {
            CheckOutcome::Passed { detail } => format!("✓ Database schema: current ({detail})"),
            CheckOutcome::Failed { detail } | CheckOutcome::Unavailable { detail } => {
                format!("✗ Database schema: FAILED ({detail})")
            }
        },
        DoctorCheck::WorkspaceDirectory { path, exists } => format!(
            "{} Workspace directory: {} ({})",
            if *exists { "✓" } else { "✗" },
            path.display(),
            if *exists { "exists" } else { "does not exist" }
        ),
        DoctorCheck::CaddyManagedDirectory { path, exists } => format!(
            "{} Caddy managed directory: {} ({})",
            if *exists { "✓" } else { "✗" },
            path.display(),
            if *exists { "exists" } else { "does not exist" }
        ),
        DoctorCheck::Caddyfile { path, exists } => format!(
            "{} Caddyfile: {} ({})",
            if *exists { "✓" } else { "✗" },
            path.display(),
            if *exists { "exists" } else { "does not exist" }
        ),
        DoctorCheck::CaddyConfiguration(outcome) => {
            format_doctor_outcome("Caddy configuration", outcome, "valid")
        }
        DoctorCheck::Git(outcome) => format_command_availability("Git", outcome),
        DoctorCheck::Podman(outcome) => format_command_availability("Podman", outcome),
        DoctorCheck::ActiveOciImage { image, outcome } => match outcome {
            CheckOutcome::Passed { .. } => format!("✓ Active OCI image: {image} (pullable)"),
            CheckOutcome::Failed { detail } | CheckOutcome::Unavailable { detail } => {
                format!("✗ Active OCI image: {image} (FAILED: {detail})")
            }
        },
        DoctorCheck::ActiveOciImages(outcome) => match outcome {
            CheckOutcome::Passed { .. } => unreachable!(),
            CheckOutcome::Failed { detail } | CheckOutcome::Unavailable { detail } => {
                format!("✗ Active OCI images: FAILED ({detail})")
            }
        },
        DoctorCheck::ActiveLocalImage => "- Active local image: skipped".to_owned(),
        DoctorCheck::DiskSpace { path, outcome } => match outcome {
            CheckOutcome::Passed { .. } => {
                format!("✓ Disk space: {} (at least 1 GiB free)", path.display())
            }
            CheckOutcome::Failed { .. } => {
                format!("✗ Disk space: {} (less than 1 GiB free)", path.display())
            }
            CheckOutcome::Unavailable { .. } => {
                format!("✗ Disk space: {} (unable to inspect)", path.display())
            }
        },
        DoctorCheck::PodmanRootless(outcome) => {
            format_doctor_outcome("Podman rootless", outcome, "OK")
        }
        DoctorCheck::PodmanQuadletUserGenerator { path } => match path {
            Some(path) => format!("✓ Podman Quadlet user generator: {}", path.display()),
            None => {
                "✗ Podman Quadlet user generator: not found (install Podman >= 4.4 or Debian 13)"
                    .to_owned()
            }
        },
        DoctorCheck::Caddy(outcome) => format_command_availability("Caddy", outcome),
    }
}

fn format_doctor_outcome(name: &str, outcome: &CheckOutcome, success: &str) -> String {
    match outcome {
        CheckOutcome::Passed { .. } => format!("✓ {name}: {success}"),
        CheckOutcome::Failed { detail } | CheckOutcome::Unavailable { detail } => {
            format!("✗ {name}: FAILED ({detail})")
        }
    }
}

fn format_command_availability(name: &str, outcome: &CheckOutcome) -> String {
    match outcome {
        CheckOutcome::Passed { detail } => format!("✓ {name}: {detail}"),
        CheckOutcome::Failed { .. } => format!("✗ {name}: command failed"),
        CheckOutcome::Unavailable { detail } => format!("✗ {name}: not found ({detail})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pneuma::domain::application::{ApplicationName, DesiredRuntimeState};
    use pneuma::domain::deployment::{Deployment, DeploymentFailure, DeploymentFailureCode};
    use pneuma::domain::git::CommitSha;
    use pneuma::domain::identity::{ApplicationId, DeploymentId, ReleaseId, RuntimeInstanceId};
    use pneuma::domain::release::{OciArtifact, Release};
    use pneuma::domain::runtime::{ContainerId, ObservedRuntimeState};

    fn observation(
        desired_runtime_state: DesiredRuntimeState,
        observed_runtime_state: ObservedRuntimeState,
    ) -> RuntimeObservation {
        RuntimeObservation {
            desired_runtime_state,
            observed_runtime_state,
            runtime_id: RuntimeInstanceId::new("0f8d3a2c41b64d7e9a0c5b6e1f2d3a4b").unwrap(),
            container_id: ContainerId::from("container-1".to_owned()),
            observed_endpoint: None,
        }
    }

    fn application_name() -> ApplicationName {
        ApplicationName::new("portal").unwrap()
    }

    #[test]
    fn labels_every_desired_runtime_state() {
        assert_eq!(
            desired_runtime_state_label(DesiredRuntimeState::Running),
            "Running"
        );
        assert_eq!(
            desired_runtime_state_label(DesiredRuntimeState::Stopped),
            "Stopped"
        );
    }

    #[test]
    fn labels_every_observed_runtime_state() {
        assert_eq!(
            observed_runtime_state_label(&ObservedRuntimeState::Missing),
            "Missing"
        );
        assert_eq!(
            observed_runtime_state_label(&ObservedRuntimeState::Created),
            "Created"
        );
        assert_eq!(
            observed_runtime_state_label(&ObservedRuntimeState::Starting),
            "Starting"
        );
        assert_eq!(
            observed_runtime_state_label(&ObservedRuntimeState::Running),
            "Running"
        );
        assert_eq!(
            observed_runtime_state_label(&ObservedRuntimeState::Stopping),
            "Stopping"
        );
        assert_eq!(
            observed_runtime_state_label(&ObservedRuntimeState::Stopped),
            "Stopped"
        );
        assert_eq!(
            observed_runtime_state_label(&ObservedRuntimeState::Failed),
            "Failed"
        );
    }

    #[test]
    fn unknown_observed_states_keep_the_structured_debug_representation() {
        assert_eq!(
            observed_runtime_state_label(&ObservedRuntimeState::Unknown {
                status: "weird".to_owned(),
            }),
            "Unknown { status: \"weird\" }"
        );
    }

    #[test]
    fn labels_every_deployment_type_and_status() {
        assert_eq!(deployment_type_label(DeploymentType::Deploy), "Deploy");
        assert_eq!(deployment_type_label(DeploymentType::Rollback), "Rollback");
        assert_eq!(
            deployment_status_label(DeploymentStatus::Pending),
            "Pending"
        );
        assert_eq!(
            deployment_status_label(DeploymentStatus::Starting),
            "Starting"
        );
        assert_eq!(
            deployment_status_label(DeploymentStatus::Verifying),
            "Verifying"
        );
        assert_eq!(
            deployment_status_label(DeploymentStatus::Activating),
            "Activating"
        );
        assert_eq!(
            deployment_status_label(DeploymentStatus::Succeeded),
            "Succeeded"
        );
        assert_eq!(deployment_status_label(DeploymentStatus::Failed), "Failed");
    }

    #[test]
    fn labels_every_visibility() {
        assert_eq!(visibility_label(Visibility::Public), "Public");
        assert_eq!(visibility_label(Visibility::Internal), "Internal");
    }

    #[test]
    fn deployment_history_renders_explicit_type_and_status_labels() {
        let id = |suffix: u8| DeploymentId::new(&format!("{}{suffix:x}", "a".repeat(31))).unwrap();
        let digest = format!("sha256:{}", "a".repeat(64));
        let sha = CommitSha::new(&"b".repeat(40)).unwrap();
        let succeeded = DeploymentHistory {
            deployment: Deployment {
                id: id(1),
                application_id: ApplicationId::new(&"c".repeat(32)).unwrap(),
                release_id: ReleaseId::new(&"d".repeat(32)).unwrap(),
                deployment_type: DeploymentType::Deploy,
                lifecycle: DeploymentLifecycle::Succeeded {
                    finished_at: "2026-09-02T10:00:00Z".to_owned(),
                },
                source_revision: Some(sha.clone()),
                requested_at: "2026-09-02T09:59:00Z".to_owned(),
                started_at: Some("2026-09-02T09:59:30Z".to_owned()),
            },
            release: Release {
                id: ReleaseId::new(&"d".repeat(32)).unwrap(),
                application_id: ApplicationId::new(&"c".repeat(32)).unwrap(),
                artifact: OciArtifact::new("registry.example/team/service", &digest).unwrap(),
                created_at: "2026-09-02T09:00:00Z".to_owned(),
            },
            is_active: true,
        };
        let failed = DeploymentHistory {
            deployment: Deployment {
                id: id(2),
                application_id: ApplicationId::new(&"c".repeat(32)).unwrap(),
                release_id: ReleaseId::new(&"d".repeat(32)).unwrap(),
                deployment_type: DeploymentType::Rollback,
                lifecycle: DeploymentLifecycle::Failed {
                    failure: DeploymentFailure {
                        code: DeploymentFailureCode::RuntimeStart,
                        stage: DeploymentStatus::Starting,
                        message: "start rejected".to_owned(),
                        finished_at: "2026-09-02T11:00:00Z".to_owned(),
                    },
                },
                source_revision: None,
                requested_at: "2026-09-02T09:59:00Z".to_owned(),
                started_at: None,
            },
            release: succeeded.release.clone(),
            is_active: false,
        };

        let rendered = deployment_history(&application_name(), &[succeeded, failed]);
        let rows: Vec<&str> = rendered.lines().collect();
        assert_eq!(rows[0], "Deployments for portal:");
        assert_eq!(
            rows[2],
            format!(
                "{}\tDeploy\t{digest}\t{}\tSucceeded\t2026-09-02T09:59:30Z\t2026-09-02T10:00:00Z\tyes\t-",
                id(1),
                sha.as_str()
            )
        );
        assert_eq!(
            rows[3],
            format!(
                "{}\tRollback\t{digest}\t-\tFailed\t-\t2026-09-02T11:00:00Z\tno\truntime_start_failed:starting:start rejected",
                id(2)
            )
        );
    }

    #[test]
    fn application_status_renders_heading_with_runtime_fields() {
        let rendered = application_status(
            &application_name(),
            &observation(DesiredRuntimeState::Running, ObservedRuntimeState::Running),
        );
        assert_eq!(
            rendered,
            "Application: portal\n\
             Desired state: Running\n\
             Observed state: Running\n\
             Runtime: 0f8d3a2c41b64d7e9a0c5b6e1f2d3a4b\n\
             Container: container-1"
        );
    }

    #[test]
    fn application_stopped_renders_heading_with_lifecycle_states() {
        let rendered = application_stopped(
            &application_name(),
            &observation(DesiredRuntimeState::Stopped, ObservedRuntimeState::Stopped),
        );
        assert_eq!(
            rendered,
            "Stopped portal\nDesired state: Stopped\nObserved state: Stopped"
        );
    }

    #[test]
    fn application_started_renders_heading_with_lifecycle_states() {
        let rendered = application_started(
            &application_name(),
            &observation(DesiredRuntimeState::Running, ObservedRuntimeState::Running),
        );
        assert_eq!(
            rendered,
            "Started portal\nDesired state: Running\nObserved state: Running"
        );
    }

    #[test]
    fn empty_system_list_renders_no_output() {
        assert_eq!(system_list(&[]), "");
    }

    #[test]
    fn database_restore_names_both_paths() {
        let rendered = database_restore(
            Path::new("/tmp/restore.sqlite3"),
            Path::new("/tmp/pre-restore.sqlite3"),
        );
        assert!(rendered.contains("/tmp/restore.sqlite3"));
        assert!(rendered.contains("/tmp/pre-restore.sqlite3"));
    }
}
