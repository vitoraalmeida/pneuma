use crate::domain::application::{ApplicationName, ApplicationSummary};
use crate::domain::deployment::DeploymentHistory;
use crate::domain::system::System;
use crate::use_cases::application::{ApplicationCatalogEntry, RuntimeObservation};
use crate::use_cases::exposure::ExposureChange;
use crate::use_cases::reconciliation::ReconciliationResult;
use crate::use_cases::system::SystemDetails;

/// Typed result of one executed command.
#[derive(Debug)]
pub enum CommandResult {
    SystemCreated(System),
    Systems(Vec<System>),
    SystemDetails(SystemDetails),
    ApplicationImported(ApplicationSummary),
    Applications(Vec<ApplicationCatalogEntry>),
    ApplicationDeployments {
        application_name: ApplicationName,
        deployments: Vec<DeploymentHistory>,
    },
    ApplicationStatus {
        application_name: ApplicationName,
        observation: RuntimeObservation,
    },
    ApplicationStopped {
        application_name: ApplicationName,
        observation: RuntimeObservation,
    },
    ApplicationStarted {
        application_name: ApplicationName,
        observation: RuntimeObservation,
    },
    ExposureChanged {
        application_name: ApplicationName,
        change: ExposureChange,
    },
    Reconciled {
        application_name: ApplicationName,
        result: ReconciliationResult,
    },
}
