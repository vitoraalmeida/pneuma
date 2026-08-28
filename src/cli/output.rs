use std::fmt::Write as _;
use std::path::Path;

use pneuma::domain::application::{ApplicationName, ApplicationSummary};
use pneuma::domain::deployment::{DeploymentHistory, DeploymentLifecycle};
use pneuma::domain::exposure::Visibility;
use pneuma::domain::git::CommitSha;
use pneuma::domain::system::System;
use pneuma::use_cases::application::RuntimeObservation;
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

pub(crate) fn application_list(entries: &[(ApplicationSummary, bool)]) -> String {
    entries
        .iter()
        .map(|(application, deployed)| {
            let deployment_status = if *deployed {
                "Deployed"
            } else {
                "Not deployed"
            };
            format!("{}\tRegistered\t{deployment_status}", application.name)
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

pub(crate) fn doctor_connection_failure(database_path: &Path) -> String {
    format!(
        "✗ Database connection: FAILED (unable to open database at {})\n\nSome checks failed. Please review the output above.",
        database_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_system_list_renders_no_output() {
        assert_eq!(system_list(&[]), "");
    }

    #[test]
    fn doctor_failure_names_the_database_path() {
        let rendered = doctor_connection_failure(Path::new("/tmp/pneuma.sqlite3"));
        assert!(rendered.contains("/tmp/pneuma.sqlite3"));
        assert!(rendered.contains("Some checks failed"));
    }
}
