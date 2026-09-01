use std::fmt::Write as _;
use std::path::Path;

use pneuma::adapters::diagnostics::{CheckOutcome, DoctorCheck, DoctorReport};
use pneuma::domain::application::{ApplicationName, ApplicationSummary};
use pneuma::domain::deployment::{DeploymentHistory, DeploymentLifecycle};
use pneuma::domain::exposure::Visibility;
use pneuma::domain::git::CommitSha;
use pneuma::domain::system::System;
use pneuma::use_cases::application::{ApplicationCatalogEntry, RuntimeObservation};
use pneuma::use_cases::deployment::DeploymentResult;
use pneuma::use_cases::exposure::ExposureChange;
use pneuma::use_cases::reconciliation::ReconciliationResult;
use pneuma::use_cases::system::SystemDetails;

// Renders command results as presentation strings so handlers stay orchestration-only.

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
        "Desired state: {:?}\nObserved state: {:?}\nRuntime: {}\nContainer: {}",
        observation.desired_runtime_state,
        observation.observed_runtime_state,
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
        "Desired state: {:?}\nObserved state: {:?}",
        observation.desired_runtime_state, observation.observed_runtime_state
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
            "{}\t{:?}\t{}\t{}\t{:?}\t{}\t{}\t{}\t{}",
            deployment.deployment.id,
            deployment.deployment.deployment_type,
            deployment.release.artifact.digest(),
            source,
            deployment.deployment.status(),
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
    match change.visibility {
        Visibility::Public => {
            let mut output = format!("Visibility for {application_name}: Public");
            if let Some(domain) = &change.domain {
                let _ = write!(output, "\nDomain: {domain}");
            }
            output
        }
        Visibility::Internal => format!("Visibility for {application_name}: Internal"),
    }
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
    use pneuma::domain::identity::RuntimeInstanceId;
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
