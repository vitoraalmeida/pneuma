use thiserror::Error;

use pneuma::adapters::database::DatabaseError;
use pneuma::adapters::stores::application_store::ApplicationStoreError;
use pneuma::adapters::stores::deployment_store::DeploymentStoreError;
use pneuma::control::ControlError;
use pneuma::domain::deployment::DeploymentFailureCode;
use pneuma::domain::release::InvalidOciArtifact;
use pneuma::domain::system::InvalidSystemName;
use pneuma::use_cases::application::{
    ApplicationLookupError, ImportError, RemoteImportError, RuntimeLifecycleError,
};
use pneuma::use_cases::ci::CiDispatchError;
use pneuma::use_cases::deployment::{
    CandidateCleanupError, CreateDeploymentError, DeployBranchError, DeployOciError,
    DeployReleaseError, RollbackError, TransitionDeploymentError,
};
use pneuma::use_cases::exposure::ExposureChangeError;
use pneuma::use_cases::reconciliation::ReconciliationReadError;
use pneuma::use_cases::release::CreateReleaseError;

/// Presentation-level class of a CLI failure, mapped to one process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliErrorClass {
    /// Persistence, database, or internal failures without a more specific cause.
    Failure,
    /// The command input itself was rejected before any effect.
    Usage,
    /// A named resource required by the command does not exist.
    NotFound,
    /// Persisted state does not allow the operation or changed concurrently.
    Conflict,
    /// Git, OCI, Podman, systemd, Caddy, or health-check integration failed.
    External,
}

impl CliErrorClass {
    pub(crate) fn exit_code(self) -> u8 {
        match self {
            Self::Failure => 1,
            Self::Usage => 2,
            Self::NotFound => 3,
            Self::Conflict => 4,
            Self::External => 5,
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum CliError {
    #[error(transparent)]
    Database { source: DatabaseError },
    #[error(transparent)]
    Import { source: RemoteImportError },
    #[error(transparent)]
    InvalidSystemName { source: InvalidSystemName },
    #[error("failed to list applications: {source}")]
    List { source: ApplicationStoreError },
    #[error("failed to load application: {source}")]
    ApplicationLookup { source: ApplicationStoreError },
    #[error("failed to list deployments: {source}")]
    ListDeployments { source: DeploymentStoreError },
    #[error("application `{application_name}` was not found")]
    ApplicationNotFound { application_name: String },
    #[error(transparent)]
    ApplicationRuntime { source: Box<RuntimeLifecycleError> },
    #[error(transparent)]
    DeployOci { source: Box<DeployOciError> },
    #[error(transparent)]
    InvalidOciArtifact { source: InvalidOciArtifact },
    #[error(transparent)]
    DeployBranch { source: Box<DeployBranchError> },
    #[error(transparent)]
    Rollback { source: RollbackError },
    #[error(transparent)]
    VisibilitySet { source: ExposureChangeError },
    #[error("failed to create system: {source}")]
    SystemCreate { source: rusqlite::Error },
    #[error("failed to list systems: {source}")]
    SystemList { source: rusqlite::Error },
    #[error(transparent)]
    SystemShow {
        source: pneuma::use_cases::system::ShowError,
    },
    #[error(transparent)]
    CiDispatch { source: CiDispatchError },
    #[error(transparent)]
    Reconcile { source: ReconciliationReadError },
    #[error("one or more diagnostic checks failed")]
    Doctor,
    #[error("either --image or --branch must be specified")]
    MissingDeployOption,
    #[error("the terminal interface requires interactive stdin and stdout")]
    TuiRequiresTerminal,
    #[error("terminal interface failed: {source}")]
    TuiTerminal { source: std::io::Error },
}

impl CliError {
    // Maps a boundary failure onto the CLI's presentation error vocabulary,
    // keeping messages and exit-code classes identical.
    pub(crate) fn from_control(source: ControlError) -> CliError {
        match source {
            ControlError::Database { source } => CliError::Database { source },
            ControlError::InvalidSystemName { source } => CliError::InvalidSystemName { source },
            ControlError::SystemCreate { source } => CliError::SystemCreate { source },
            ControlError::SystemList { source } => CliError::SystemList { source },
            ControlError::SystemShow { source } => CliError::SystemShow { source },
            ControlError::Import { source } => CliError::Import { source },
            ControlError::ListApplications { source } => CliError::List { source },
            ControlError::ApplicationLookup { source } => match source {
                ApplicationLookupError::NotFound { application_name } => {
                    CliError::ApplicationNotFound { application_name }
                }
                ApplicationLookupError::Store { source } => CliError::ApplicationLookup { source },
            },
            ControlError::ListDeployments { source } => CliError::ListDeployments { source },
            ControlError::RuntimeStatus { source }
            | ControlError::RuntimeStop { source }
            | ControlError::RuntimeStart { source } => CliError::ApplicationRuntime {
                source: Box::new(source),
            },
            ControlError::InvalidOciArtifact { source } => CliError::InvalidOciArtifact { source },
            ControlError::DeployOci { source } => CliError::DeployOci {
                source: Box::new(source),
            },
            ControlError::DeployBranch { source } => CliError::DeployBranch {
                source: Box::new(source),
            },
            ControlError::Rollback { source } => CliError::Rollback { source },
            ControlError::VisibilitySet { source } => CliError::VisibilitySet { source },
            ControlError::Reconcile { source } => CliError::Reconcile { source },
            ControlError::DoctorConnection { source, .. } => CliError::Database { source },
        }
    }

    /// Classifies the failure for message/exit-code presentation without erasing context.
    pub(crate) fn class(&self) -> CliErrorClass {
        match self {
            Self::InvalidOciArtifact { .. }
            | Self::MissingDeployOption
            | Self::TuiRequiresTerminal => CliErrorClass::Usage,
            Self::ApplicationNotFound { .. } => CliErrorClass::NotFound,
            Self::Import { source } => classify_remote_import(source),
            Self::InvalidSystemName { .. } => CliErrorClass::Usage,
            Self::ApplicationRuntime { source } => classify_runtime_lifecycle(source),
            Self::DeployOci { source } => classify_deploy_oci(source),
            Self::DeployBranch { source } => classify_deploy_branch(source),
            Self::Rollback { source } => classify_rollback(source),
            Self::VisibilitySet { source } => classify_exposure_change(source),
            Self::SystemShow { source } => match source {
                pneuma::use_cases::system::ShowError::NotFound { .. } => CliErrorClass::NotFound,
                pneuma::use_cases::system::ShowError::ApplicationStore { .. }
                | pneuma::use_cases::system::ShowError::Persistence { .. } => {
                    CliErrorClass::Failure
                }
            },
            Self::CiDispatch { .. } => CliErrorClass::Usage,
            Self::Reconcile { source } => classify_reconciliation_read(source),
            Self::Database { source } => match source {
                // Database-wide lock contention is a caller-visible conflict.
                DatabaseError::DatabaseBusy { .. } => CliErrorClass::Conflict,
                DatabaseError::Open { .. }
                | DatabaseError::Configure { .. }
                | DatabaseError::Initialize { .. }
                | DatabaseError::IncompatibleSchema { .. }
                | DatabaseError::BackupDestinationExists { .. }
                | DatabaseError::BackupDestinationParent { .. }
                | DatabaseError::Backup { .. }
                | DatabaseError::RestoreSource { .. }
                | DatabaseError::RestoreIntegrity { .. }
                | DatabaseError::DatabaseLock { .. }
                | DatabaseError::RestoreReplace { .. } => CliErrorClass::Failure,
            },
            Self::List { .. }
            | Self::ApplicationLookup { .. }
            | Self::ListDeployments { .. }
            | Self::SystemCreate { .. }
            | Self::SystemList { .. }
            | Self::Doctor
            | Self::TuiTerminal { .. } => CliErrorClass::Failure,
        }
    }
}

fn classify_deploy_branch(source: &DeployBranchError) -> CliErrorClass {
    match source {
        DeployBranchError::ResolveBranch { .. } | DeployBranchError::ResolveImageDigest { .. } => {
            CliErrorClass::External
        }
        DeployBranchError::DeployOci { source } => classify_deploy_oci(source),
        // Lock open/acquire failures are infrastructure failures; only the
        // dedicated busy variants represent real contention.
        DeployBranchError::ApplicationLock { .. } => CliErrorClass::Failure,
        DeployBranchError::ApplicationBusy { .. } => CliErrorClass::Conflict,
        // Missing persisted source or delivery configuration means the
        // application's recorded state does not allow branch deployment.
        DeployBranchError::NoSourceConfiguration { .. }
        | DeployBranchError::NoDeliveryConfiguration { .. } => CliErrorClass::Conflict,
        DeployBranchError::NoDefaultBranch { .. } => CliErrorClass::Usage,
        DeployBranchError::SourceConfiguration { .. } => CliErrorClass::Failure,
    }
}

fn classify_deploy_oci(source: &DeployOciError) -> CliErrorClass {
    match source {
        DeployOciError::RepositoryMismatch { .. } => CliErrorClass::Usage,
        DeployOciError::PullImage { .. } => CliErrorClass::External,
        DeployOciError::CreateRelease { source } => classify_create_release(source),
        DeployOciError::DeployRelease { source } => classify_deploy_release(source),
        DeployOciError::ApplicationLock { .. } => CliErrorClass::Failure,
        DeployOciError::ApplicationBusy { .. } => CliErrorClass::Conflict,
        DeployOciError::NoDeliveryConfiguration { .. } => CliErrorClass::Conflict,
        DeployOciError::DeliveryConfiguration { .. } => CliErrorClass::Failure,
    }
}

// Classifies a nested release workflow failure by its typed cause.
fn classify_create_release(source: &CreateReleaseError) -> CliErrorClass {
    match source {
        CreateReleaseError::ApplicationNotFound { .. } => CliErrorClass::NotFound,
        CreateReleaseError::ApplicationBusy { .. } => CliErrorClass::Conflict,
        CreateReleaseError::ApplicationLock { .. } => CliErrorClass::Failure,
        CreateReleaseError::ApplicationStore { .. }
        | CreateReleaseError::ReleaseStore { .. }
        | CreateReleaseError::Persistence { .. } => CliErrorClass::Failure,
    }
}

// Classifies a nested deployment-record failure by its typed cause.
fn classify_create_deployment(source: &CreateDeploymentError) -> CliErrorClass {
    match source {
        CreateDeploymentError::ApplicationNotFound { .. }
        | CreateDeploymentError::ReleaseNotFound { .. } => CliErrorClass::NotFound,
        CreateDeploymentError::ActiveDeployment { .. }
        | CreateDeploymentError::AlreadyActive { .. }
        | CreateDeploymentError::ApplicationBusy { .. } => CliErrorClass::Conflict,
        CreateDeploymentError::ApplicationLock { .. }
        | CreateDeploymentError::Persistence { .. } => CliErrorClass::Failure,
    }
}

// Classifies a nested release execution failure: missing resources are absent,
// state divergences are conflicts, and each persisted failure stage carries its
// own external or generic semantics.
fn classify_deploy_release(source: &DeployReleaseError) -> CliErrorClass {
    match source {
        DeployReleaseError::ApplicationNotFound { .. } => CliErrorClass::NotFound,
        DeployReleaseError::PublicApplication { .. } => CliErrorClass::Conflict,
        DeployReleaseError::LoadApplication { .. } => CliErrorClass::Failure,
        DeployReleaseError::CreateDeployment { source } => classify_create_deployment(source),
        DeployReleaseError::DeploymentFailed { code, .. } => {
            classify_deployment_failure_code(*code)
        }
        DeployReleaseError::RecordFailure { source, .. } => classify_transition_deployment(source),
        DeployReleaseError::Cleanup { source, .. } => classify_candidate_cleanup(source),
    }
}

// Maps each persisted failure stage onto its semantic exit class: stages that
// surfaced through Podman, systemd, Caddy, or a health probe are external, and
// every orchestration or persistence stage remains a generic failure.
fn classify_deployment_failure_code(code: DeploymentFailureCode) -> CliErrorClass {
    match code {
        DeploymentFailureCode::RuntimeUnitCreation
        | DeploymentFailureCode::RuntimeUnitReload
        | DeploymentFailureCode::RuntimeStart
        | DeploymentFailureCode::RuntimeResolution
        | DeploymentFailureCode::RuntimeObservation
        | DeploymentFailureCode::HealthCheck
        | DeploymentFailureCode::CaddyMaterialization
        | DeploymentFailureCode::ExternalHealthCheck => CliErrorClass::External,
        DeploymentFailureCode::TestGate
        | DeploymentFailureCode::RuntimeReconciliation
        | DeploymentFailureCode::PublicConfigurationMissing
        | DeploymentFailureCode::RuntimePortAllocation
        | DeploymentFailureCode::RuntimeRegistration
        | DeploymentFailureCode::RuntimePortPersistence
        | DeploymentFailureCode::DeploymentTransition
        | DeploymentFailureCode::ExposurePreparation
        | DeploymentFailureCode::CandidatePromotion
        | DeploymentFailureCode::OperationInterrupted => CliErrorClass::Failure,
    }
}

// Classifies a failure-recording divergence by its typed transition cause.
fn classify_transition_deployment(source: &TransitionDeploymentError) -> CliErrorClass {
    match source {
        TransitionDeploymentError::DeploymentNotFound { .. } => CliErrorClass::NotFound,
        TransitionDeploymentError::Conflict { .. }
        | TransitionDeploymentError::CannotFail { .. }
        | TransitionDeploymentError::InvalidTransition { .. } => CliErrorClass::Conflict,
        TransitionDeploymentError::InvalidPersistedStatus { .. }
        | TransitionDeploymentError::InvalidPersistedType { .. }
        | TransitionDeploymentError::InvalidFailure { .. }
        | TransitionDeploymentError::Persistence { .. } => CliErrorClass::Failure,
    }
}

// Classifies candidate cleanup divergence: systemd and Podman effects are
// external, runtime divergence is a conflict, and local persistence is failure.
fn classify_candidate_cleanup(source: &CandidateCleanupError) -> CliErrorClass {
    match source {
        CandidateCleanupError::Supervision { .. }
        | CandidateCleanupError::RemoveContainer { .. }
        | CandidateCleanupError::ContainerNotRemoved { .. } => CliErrorClass::External,
        CandidateCleanupError::RuntimeChanged { .. } => CliErrorClass::Conflict,
        CandidateCleanupError::ReleasePort { .. } | CandidateCleanupError::Persistence { .. } => {
            CliErrorClass::Failure
        }
    }
}

// Classifies import failures, separating rejected input from external and persistence causes.
fn classify_remote_import(source: &RemoteImportError) -> CliErrorClass {
    match source {
        RemoteImportError::InvalidRepository | RemoteImportError::InvalidSystemName { .. } => {
            CliErrorClass::Usage
        }
        RemoteImportError::Clone { .. } => CliErrorClass::External,
        RemoteImportError::Workspace { .. } => CliErrorClass::Failure,
        RemoteImportError::Import { source } => match source {
            ImportError::Manifest { .. } | ImportError::SystemRequired => CliErrorClass::Usage,
            ImportError::ApplicationNotFound { .. } => CliErrorClass::NotFound,
            ImportError::Persistence { .. } => CliErrorClass::Failure,
        },
    }
}

fn classify_runtime_lifecycle(source: &RuntimeLifecycleError) -> CliErrorClass {
    match source {
        RuntimeLifecycleError::NotDeployed { .. }
        | RuntimeLifecycleError::ContainerMissing { .. } => CliErrorClass::NotFound,
        RuntimeLifecycleError::RuntimeChanged { .. }
        | RuntimeLifecycleError::ApplicationBusy { .. } => CliErrorClass::Conflict,
        RuntimeLifecycleError::ApplicationLock { .. } => CliErrorClass::Failure,
        RuntimeLifecycleError::Observe { .. }
        | RuntimeLifecycleError::Control { .. }
        | RuntimeLifecycleError::Supervision { .. } => CliErrorClass::External,
        // Invalid persisted values, store access, and direct persistence are
        // generic failures, not caller-visible conflicts.
        RuntimeLifecycleError::InvalidDesiredState { .. }
        | RuntimeLifecycleError::ApplicationStore { .. }
        | RuntimeLifecycleError::Persistence { .. } => CliErrorClass::Failure,
    }
}

fn classify_rollback(source: &RollbackError) -> CliErrorClass {
    match source {
        RollbackError::ApplicationNotFound { .. } => CliErrorClass::NotFound,
        RollbackError::NoPreviousDeployment { .. } => CliErrorClass::Conflict,
        RollbackError::PullImage { .. } => CliErrorClass::External,
        RollbackError::ApplicationLock { .. } => CliErrorClass::Failure,
        RollbackError::ApplicationBusy { .. } => CliErrorClass::Conflict,
        RollbackError::LoadTarget { .. } => CliErrorClass::Failure,
        RollbackError::DeployRelease { source } => classify_deploy_release(source),
    }
}

fn classify_exposure_change(source: &ExposureChangeError) -> CliErrorClass {
    match source {
        ExposureChangeError::ApplicationNotFound { .. }
        | ExposureChangeError::NoActiveRuntime { .. } => CliErrorClass::NotFound,
        // A public exposure without a domain is a recorded-state rejection,
        // not rejected command input.
        ExposureChangeError::DomainRequired { .. } => CliErrorClass::Conflict,
        ExposureChangeError::ExposureChanged { .. }
        | ExposureChangeError::RuntimeNotRunning { .. }
        | ExposureChangeError::ApplicationBusy { .. } => CliErrorClass::Conflict,
        ExposureChangeError::ApplicationLock { .. } => CliErrorClass::Failure,
        // Invalid persisted visibility and other invalid persisted exposure
        // values are database-integrity failures, not rejected command input.
        ExposureChangeError::InvalidVisibility { .. } => CliErrorClass::Failure,
        ExposureChangeError::ObserveFailed { .. }
        | ExposureChangeError::InvalidObservedEndpoint { .. }
        | ExposureChangeError::MaterializeFailed { .. }
        | ExposureChangeError::RemoveFragmentFailed { .. }
        | ExposureChangeError::ExternalHealthFailed { .. } => CliErrorClass::External,
        ExposureChangeError::InvalidMaterializationState { .. }
        | ExposureChangeError::InvalidExposure { .. }
        | ExposureChangeError::InvalidConfigurationVersion
        | ExposureChangeError::InvalidDiagnostic
        | ExposureChangeError::Store { .. }
        | ExposureChangeError::RuntimeStore { .. }
        | ExposureChangeError::ApplicationStore { .. }
        | ExposureChangeError::Persistence { .. } => CliErrorClass::Failure,
    }
}

fn classify_reconciliation_read(source: &ReconciliationReadError) -> CliErrorClass {
    match source {
        ReconciliationReadError::ApplicationNotFound { .. } => CliErrorClass::NotFound,
        // Reconciliation reports real contention as a successful `Deferred`
        // result, so this wrapper can only be a lock infrastructure failure.
        ReconciliationReadError::OperationLock { .. } => CliErrorClass::Failure,
        ReconciliationReadError::ObserveContainer { .. }
        | ReconciliationReadError::ObserveNamedContainer { .. }
        | ReconciliationReadError::ObserveQuadlet { .. }
        | ReconciliationReadError::ObserveCaddy { .. } => CliErrorClass::External,
        // Unconverged recorded state keeps its generic classification: it is
        // neither store corruption nor caller-visible contention.
        ReconciliationReadError::Application { .. }
        | ReconciliationReadError::Deployment { .. }
        | ReconciliationReadError::Release { .. }
        | ReconciliationReadError::Runtime { .. }
        | ReconciliationReadError::Exposure { .. }
        | ReconciliationReadError::InvalidExpectedPort { .. }
        | ReconciliationReadError::NotConverged { .. } => CliErrorClass::Failure,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;

    use super::*;
    use pneuma::adapters::database::DatabaseError;
    use pneuma::adapters::git_source::{CloneRepositoryError, ResolveBranchError};
    use pneuma::adapters::health_check_external::ExternalHealthCheckError;
    use pneuma::adapters::local_runtime::PodmanError;
    use pneuma::adapters::oci_image::PullImageError;
    use pneuma::adapters::stores::application_store::ApplicationStoreError;
    use pneuma::use_cases::ci::CiDispatchError;
    use pneuma::use_cases::system::ShowError;

    // Table-driven classification coverage: one representative error per major
    // command family, with every exit class exercised. Nested operation layering
    // (branch deploy -> OCI deploy) is classified through the shared helper.
    #[test]
    fn representative_errors_classify_per_command_family() {
        let cases: Vec<(&str, CliError, CliErrorClass)> = vec![
            // Import family.
            (
                "import: rejected repository input",
                CliError::Import {
                    source: RemoteImportError::InvalidRepository,
                },
                CliErrorClass::Usage,
            ),
            (
                "import: missing system requirement",
                CliError::Import {
                    source: RemoteImportError::Import {
                        source: ImportError::SystemRequired,
                    },
                },
                CliErrorClass::Usage,
            ),
            (
                "import: nested application absence",
                CliError::Import {
                    source: RemoteImportError::Import {
                        source: ImportError::ApplicationNotFound {
                            application_id: "app-1".to_owned(),
                        },
                    },
                },
                CliErrorClass::NotFound,
            ),
            (
                "import: git clone failure",
                CliError::Import {
                    source: RemoteImportError::Clone {
                        source: clone_error(),
                    },
                },
                CliErrorClass::External,
            ),
            (
                "import: workspace preparation failure",
                CliError::Import {
                    source: RemoteImportError::Workspace {
                        source: io::Error::other("disk full"),
                    },
                },
                CliErrorClass::Failure,
            ),
            (
                "import: merged persistence failure",
                CliError::Import {
                    source: RemoteImportError::Import {
                        source: ImportError::Persistence {
                            source: Box::new(store_error()),
                        },
                    },
                },
                CliErrorClass::Failure,
            ),
            // Application runtime lifecycle family.
            (
                "app runtime: not deployed",
                CliError::ApplicationRuntime {
                    source: Box::new(RuntimeLifecycleError::NotDeployed {
                        application_name: "portal".to_owned(),
                    }),
                },
                CliErrorClass::NotFound,
            ),
            (
                "app runtime: concurrent change",
                CliError::ApplicationRuntime {
                    source: Box::new(RuntimeLifecycleError::RuntimeChanged {
                        runtime_id: "runtime-1".to_owned(),
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "app runtime: podman control failure",
                CliError::ApplicationRuntime {
                    source: Box::new(RuntimeLifecycleError::Observe {
                        runtime_id: "runtime-1".to_owned(),
                        source: podman_error(),
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "app runtime: persistence failure",
                CliError::ApplicationRuntime {
                    source: Box::new(RuntimeLifecycleError::ApplicationStore {
                        source: store_error(),
                    }),
                },
                CliErrorClass::Failure,
            ),
            (
                "app runtime: invalid persisted desired state",
                CliError::ApplicationRuntime {
                    source: Box::new(RuntimeLifecycleError::InvalidDesiredState {
                        state: "paused".to_owned(),
                    }),
                },
                CliErrorClass::Failure,
            ),
            // OCI deployment family.
            (
                "deploy image: repository policy mismatch",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::RepositoryMismatch {
                        application_id: "app-1".to_owned(),
                        allowed: "registry.example".to_owned(),
                        actual: "other.example".to_owned(),
                    }),
                },
                CliErrorClass::Usage,
            ),
            (
                "deploy image: pull failure",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::PullImage {
                        source: pull_image_error(),
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "deploy image: persistence failure",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeliveryConfiguration {
                        source: store_error(),
                    }),
                },
                CliErrorClass::Failure,
            ),
            (
                "deploy image: missing delivery configuration",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::NoDeliveryConfiguration {
                        application_id: "app-1".to_owned(),
                    }),
                },
                CliErrorClass::Conflict,
            ),
            // Branch deployment family, including nested OCI layering.
            (
                "deploy branch: missing source configuration",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::NoSourceConfiguration {
                        application_id: "app-1".to_owned(),
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "deploy branch: missing delivery configuration",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::NoDeliveryConfiguration {
                        application_id: "app-1".to_owned(),
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "deploy branch: missing default branch",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::NoDefaultBranch {
                        application_id: "app-1".to_owned(),
                    }),
                },
                CliErrorClass::Usage,
            ),
            (
                "deploy branch: branch resolution failure",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::ResolveBranch {
                        source: ResolveBranchError::BranchNotFound {
                            url: "https://git.example/app.git".to_owned(),
                            branch: "main".to_owned(),
                        },
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "deploy branch: nested pull failure",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::DeployOci {
                        source: DeployOciError::PullImage {
                            source: pull_image_error(),
                        },
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "deploy branch: nested input rejection",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::DeployOci {
                        source: DeployOciError::RepositoryMismatch {
                            application_id: "app-1".to_owned(),
                            allowed: "registry.example".to_owned(),
                            actual: "other.example".to_owned(),
                        },
                    }),
                },
                CliErrorClass::Usage,
            ),
            (
                "deploy branch: persistence failure",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::SourceConfiguration {
                        source: store_error(),
                    }),
                },
                CliErrorClass::Failure,
            ),
            // Rollback family.
            (
                "rollback: application not found",
                CliError::Rollback {
                    source: RollbackError::ApplicationNotFound {
                        application_id: "app-1".to_owned(),
                    },
                },
                CliErrorClass::NotFound,
            ),
            (
                "rollback: no previous deployment",
                CliError::Rollback {
                    source: RollbackError::NoPreviousDeployment {
                        application_id: "app-1".to_owned(),
                    },
                },
                CliErrorClass::Conflict,
            ),
            (
                "rollback: image pull failure",
                CliError::Rollback {
                    source: RollbackError::PullImage {
                        source: pull_image_error(),
                    },
                },
                CliErrorClass::External,
            ),
            (
                "rollback: merged load-target failure",
                CliError::Rollback {
                    source: RollbackError::LoadTarget {
                        source: Box::new(io::Error::other("disk full")),
                    },
                },
                CliErrorClass::Failure,
            ),
            // Visibility change family.
            (
                "visibility: application not found",
                CliError::VisibilitySet {
                    source: ExposureChangeError::ApplicationNotFound {
                        application_id: "app-1".to_owned(),
                    },
                },
                CliErrorClass::NotFound,
            ),
            (
                "visibility: persisted invalid visibility",
                CliError::VisibilitySet {
                    source: ExposureChangeError::InvalidVisibility {
                        visibility: "maybe".to_owned(),
                    },
                },
                CliErrorClass::Failure,
            ),
            (
                "visibility: exposure domain required",
                CliError::VisibilitySet {
                    source: ExposureChangeError::DomainRequired {
                        application_id: "app-1".to_owned(),
                    },
                },
                CliErrorClass::Conflict,
            ),
            (
                "visibility: non-loopback observed endpoint",
                CliError::VisibilitySet {
                    source: ExposureChangeError::InvalidObservedEndpoint {
                        container_id: "abc123".to_owned(),
                    },
                },
                CliErrorClass::External,
            ),
            (
                "visibility: invalid persisted materialization state",
                CliError::VisibilitySet {
                    source: ExposureChangeError::InvalidMaterializationState {
                        state: "maybe".to_owned(),
                    },
                },
                CliErrorClass::Failure,
            ),
            (
                "visibility: concurrent exposure change",
                CliError::VisibilitySet {
                    source: ExposureChangeError::ExposureChanged {
                        application_id: "app-1".to_owned(),
                    },
                },
                CliErrorClass::Conflict,
            ),
            (
                "visibility: external health failure",
                CliError::VisibilitySet {
                    source: ExposureChangeError::ExternalHealthFailed {
                        source: ExternalHealthCheckError::RequestFailed {
                            stderr: "connection refused".to_owned(),
                        },
                    },
                },
                CliErrorClass::External,
            ),
            // System family.
            (
                "system show: not found",
                CliError::SystemShow {
                    source: ShowError::NotFound {
                        system_name: "billing".to_owned(),
                    },
                },
                CliErrorClass::NotFound,
            ),
            (
                "system show: persistence failure",
                CliError::SystemShow {
                    source: ShowError::Persistence {
                        source: sqlite_error(),
                    },
                },
                CliErrorClass::Failure,
            ),
            (
                "system show: application store failure",
                CliError::SystemShow {
                    source: ShowError::ApplicationStore {
                        source: store_error(),
                    },
                },
                CliErrorClass::Failure,
            ),
            (
                "system create: persistence failure",
                CliError::SystemCreate {
                    source: sqlite_error(),
                },
                CliErrorClass::Failure,
            ),
            // CI dispatch family.
            (
                "ci dispatch: rejected command input",
                CliError::CiDispatch {
                    source: CiDispatchError::EmptyCommand,
                },
                CliErrorClass::Usage,
            ),
            // Reconciliation family.
            (
                "reconcile: application not found",
                CliError::Reconcile {
                    source: ReconciliationReadError::ApplicationNotFound {
                        application_name: "portal".to_owned(),
                    },
                },
                CliErrorClass::NotFound,
            ),
            (
                "reconcile: operation lock open failure",
                CliError::Reconcile {
                    source: ReconciliationReadError::OperationLock {
                        source: pneuma::adapters::application_lock::ApplicationLockError::Open {
                            path: "/tmp/lock".into(),
                            source: io::Error::other("locked"),
                        },
                    },
                },
                CliErrorClass::Failure,
            ),
            (
                "reconcile: container observation failure",
                CliError::Reconcile {
                    source: ReconciliationReadError::ObserveContainer {
                        source: podman_error(),
                    },
                },
                CliErrorClass::External,
            ),
            (
                "reconcile: unconverged recorded state",
                CliError::Reconcile {
                    source: ReconciliationReadError::NotConverged {
                        reason: "application has no active runtime".to_owned(),
                    },
                },
                CliErrorClass::Failure,
            ),
            // Database family (open, backup, and restore share one variant).
            (
                "database: open failure",
                CliError::Database {
                    source: DatabaseError::Open {
                        path: "/tmp/pneuma.sqlite3".into(),
                        source: sqlite_error(),
                    },
                },
                CliErrorClass::Failure,
            ),
            (
                "database: busy from another command",
                CliError::Database {
                    source: DatabaseError::DatabaseBusy {
                        path: "/tmp/pneuma.sqlite3".into(),
                    },
                },
                CliErrorClass::Conflict,
            ),
            // Read-only query family.
            (
                "deployment list: persistence failure",
                CliError::ListDeployments {
                    source: DeploymentStoreError::Stale {
                        deployment_id: "deployment-1".to_owned(),
                    },
                },
                CliErrorClass::Failure,
            ),
            // Top-level command input and diagnostics.
            (
                "app lookup by name: not found",
                CliError::ApplicationNotFound {
                    application_name: "portal".to_owned(),
                },
                CliErrorClass::NotFound,
            ),
            (
                "import: invalid system name input",
                CliError::InvalidSystemName {
                    source: pneuma::domain::system::InvalidSystemName {
                        value: "bad name".to_owned(),
                    },
                },
                CliErrorClass::Usage,
            ),
            (
                "deploy: missing image and branch",
                CliError::MissingDeployOption,
                CliErrorClass::Usage,
            ),
            (
                "deploy: invalid artifact reference",
                CliError::InvalidOciArtifact {
                    source: pneuma::domain::release::OciArtifact::parse("not-a-digest")
                        .expect_err("invalid reference must be rejected"),
                },
                CliErrorClass::Usage,
            ),
            (
                "doctor: failed diagnostic checks",
                CliError::Doctor,
                CliErrorClass::Failure,
            ),
        ];

        for (description, error, expected) in cases {
            assert_eq!(error.class(), expected, "{description}");
            assert_eq!(
                error.class().exit_code(),
                expected.exit_code(),
                "{description}"
            );
        }
    }

    fn clone_error() -> CloneRepositoryError {
        CloneRepositoryError::Execute {
            operation: "clone",
            source: io::Error::other("no network"),
        }
    }

    fn store_error() -> ApplicationStoreError {
        ApplicationStoreError::Persistence {
            source: sqlite_error(),
        }
    }

    fn sqlite_error() -> rusqlite::Error {
        rusqlite::Error::InvalidParameterName("test".to_owned())
    }

    fn podman_error() -> PodmanError {
        PodmanError::Execute {
            operation: "observing",
            source: io::Error::other("no podman"),
        }
    }

    fn pull_image_error() -> PullImageError {
        PullImageError::Pull {
            reference: "registry.example/app@sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
            stdout: String::new(),
            stderr: "denied".to_owned(),
        }
    }

    #[test]
    fn exit_codes_are_stable_per_class() {
        assert_eq!(CliErrorClass::Failure.exit_code(), 1);
        assert_eq!(CliErrorClass::Usage.exit_code(), 2);
        assert_eq!(CliErrorClass::NotFound.exit_code(), 3);
        assert_eq!(CliErrorClass::Conflict.exit_code(), 4);
        assert_eq!(CliErrorClass::External.exit_code(), 5);
    }

    // Every CLI-visible wrapper of `ApplicationLockError` is infrastructure
    // failure, while every matching `ApplicationBusy` wrapper is real
    // contention. The reconciliation `OperationLock` wrapper is a failure too,
    // because reconciliation reports contention as a successful `Deferred`.
    #[test]
    fn lock_wrappers_classify_as_failures_and_busy_wrappers_as_conflicts() {
        use pneuma::adapters::application_lock::ApplicationLockError;

        let open = || ApplicationLockError::Open {
            path: "/tmp/lock".into(),
            source: io::Error::other("is a directory"),
        };
        let acquire = || ApplicationLockError::Acquire {
            path: "/tmp/lock".into(),
            source: io::Error::other("unknown error"),
        };
        let busy = "portal".to_owned();

        let cases: Vec<(&str, CliError, CliErrorClass)> = vec![
            (
                "deploy image: lock open failure",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::ApplicationLock { source: open() }),
                },
                CliErrorClass::Failure,
            ),
            (
                "deploy image: lock acquire failure",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::ApplicationLock { source: acquire() }),
                },
                CliErrorClass::Failure,
            ),
            (
                "deploy image: real contention",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::ApplicationBusy {
                        application_id: busy.clone(),
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "deploy branch: lock open failure",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::ApplicationLock { source: open() }),
                },
                CliErrorClass::Failure,
            ),
            (
                "deploy branch: real contention",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::ApplicationBusy {
                        application_id: busy.clone(),
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "rollback: lock open failure",
                CliError::Rollback {
                    source: RollbackError::ApplicationLock { source: open() },
                },
                CliErrorClass::Failure,
            ),
            (
                "rollback: real contention",
                CliError::Rollback {
                    source: RollbackError::ApplicationBusy {
                        application_id: busy.clone(),
                    },
                },
                CliErrorClass::Conflict,
            ),
            (
                "app runtime: lock open failure",
                CliError::ApplicationRuntime {
                    source: Box::new(RuntimeLifecycleError::ApplicationLock { source: open() }),
                },
                CliErrorClass::Failure,
            ),
            (
                "app runtime: real contention",
                CliError::ApplicationRuntime {
                    source: Box::new(RuntimeLifecycleError::ApplicationBusy {
                        application_id: busy.clone(),
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "visibility: lock open failure",
                CliError::VisibilitySet {
                    source: ExposureChangeError::ApplicationLock { source: open() },
                },
                CliErrorClass::Failure,
            ),
            (
                "visibility: real contention",
                CliError::VisibilitySet {
                    source: ExposureChangeError::ApplicationBusy {
                        application_id: busy.clone(),
                    },
                },
                CliErrorClass::Conflict,
            ),
            (
                "reconcile: lock acquire failure",
                CliError::Reconcile {
                    source: ReconciliationReadError::OperationLock { source: acquire() },
                },
                CliErrorClass::Failure,
            ),
        ];

        for (description, error, expected) in cases {
            assert_eq!(error.class(), expected, "{description}");
            assert_eq!(
                error.class().exit_code(),
                expected.exit_code(),
                "{description}"
            );
        }
    }

    // Every persisted deployment failure stage has one semantic exit class:
    // stages surfaced by Podman, systemd, Caddy, or a health probe are external
    // integrations, and every orchestration or persistence stage is generic.
    #[test]
    fn deployment_failure_codes_classify_external_stages_and_generic_failures() {
        let external_codes = [
            DeploymentFailureCode::RuntimeUnitCreation,
            DeploymentFailureCode::RuntimeUnitReload,
            DeploymentFailureCode::RuntimeStart,
            DeploymentFailureCode::RuntimeResolution,
            DeploymentFailureCode::RuntimeObservation,
            DeploymentFailureCode::HealthCheck,
            DeploymentFailureCode::CaddyMaterialization,
            DeploymentFailureCode::ExternalHealthCheck,
        ];
        let generic_codes = [
            DeploymentFailureCode::TestGate,
            DeploymentFailureCode::RuntimeReconciliation,
            DeploymentFailureCode::PublicConfigurationMissing,
            DeploymentFailureCode::RuntimePortAllocation,
            DeploymentFailureCode::RuntimeRegistration,
            DeploymentFailureCode::RuntimePortPersistence,
            DeploymentFailureCode::DeploymentTransition,
            DeploymentFailureCode::ExposurePreparation,
            DeploymentFailureCode::CandidatePromotion,
            DeploymentFailureCode::OperationInterrupted,
        ];

        for code in external_codes {
            assert_eq!(
                classify_deployment_failure_code(code),
                CliErrorClass::External,
                "{}",
                code.as_str()
            );
        }
        for code in generic_codes {
            assert_eq!(
                classify_deployment_failure_code(code),
                CliErrorClass::Failure,
                "{}",
                code.as_str()
            );
        }
    }

    // Nested deployment errors delegate to the semantic classifiers of their
    // typed causes instead of collapsing into one generic failure class.
    #[test]
    fn nested_deployment_errors_classify_semantically() {
        use pneuma::adapters::port_allocator::PortAllocationError;
        use pneuma::adapters::systemd_quadlet::QuadletError;
        use pneuma::domain::deployment::{Deployment, DeploymentLifecycle, DeploymentType};
        use pneuma::domain::identity::RuntimeInstanceId;

        let runtime_id = RuntimeInstanceId::new("11111111111111111111111111111111").unwrap();

        // Nested external failures surface through every deployment entry point.
        let cases: Vec<(&str, CliError, CliErrorClass)> = vec![
            (
                "deploy image: nested systemd start failure",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::DeploymentFailed {
                            deployment_id: "deployment-1".to_owned(),
                            code: DeploymentFailureCode::RuntimeStart,
                            source: Box::new(io::Error::other("container start failed")),
                        },
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "deploy branch: nested health check failure",
                CliError::DeployBranch {
                    source: Box::new(DeployBranchError::DeployOci {
                        source: DeployOciError::DeployRelease {
                            source: DeployReleaseError::DeploymentFailed {
                                deployment_id: "deployment-1".to_owned(),
                                code: DeploymentFailureCode::HealthCheck,
                                source: Box::new(io::Error::other("unhealthy candidate")),
                            },
                        },
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "rollback: nested caddy materialization failure",
                CliError::Rollback {
                    source: RollbackError::DeployRelease {
                        source: DeployReleaseError::DeploymentFailed {
                            deployment_id: "deployment-1".to_owned(),
                            code: DeploymentFailureCode::CaddyMaterialization,
                            source: Box::new(io::Error::other("reload rejected")),
                        },
                    },
                },
                CliErrorClass::External,
            ),
            (
                "deploy image: nested missing application",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::ApplicationNotFound {
                            application_id: "portal".to_owned(),
                        },
                    }),
                },
                CliErrorClass::NotFound,
            ),
            (
                "deploy image: nested missing release",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::CreateDeployment {
                            source: CreateDeploymentError::ReleaseNotFound {
                                release_id: "release-1".to_owned(),
                            },
                        },
                    }),
                },
                CliErrorClass::NotFound,
            ),
            (
                "deploy image: nested active deployment conflict",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::CreateDeployment {
                            source: CreateDeploymentError::ActiveDeployment {
                                deployment: Box::new(Deployment {
                                    id: pneuma::domain::identity::DeploymentId::new(
                                        "22222222222222222222222222222222",
                                    )
                                    .unwrap(),
                                    application_id: pneuma::domain::identity::ApplicationId::new(
                                        "11111111111111111111111111111111",
                                    )
                                    .unwrap(),
                                    release_id: pneuma::domain::identity::ReleaseId::new(
                                        "55555555555555555555555555555555",
                                    )
                                    .unwrap(),
                                    deployment_type: DeploymentType::Deploy,
                                    lifecycle: DeploymentLifecycle::Pending,
                                    source_revision: None,
                                    requested_at: "now".to_owned(),
                                    started_at: None,
                                }),
                            },
                        },
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "deploy image: nested release operation contention",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::CreateRelease {
                        source: CreateReleaseError::ApplicationBusy {
                            application_id: "portal".to_owned(),
                        },
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "deploy image: cleanup divergence from systemd supervision",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::Cleanup {
                            deployment_id: "deployment-1".to_owned(),
                            failure: "container start failed".to_owned(),
                            source: Box::new(CandidateCleanupError::Supervision {
                                source: QuadletError::Execute {
                                    operation: "reloading",
                                    source: io::Error::other("no systemctl"),
                                },
                            }),
                        },
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "deploy image: cleanup divergence from podman removal",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::Cleanup {
                            deployment_id: "deployment-1".to_owned(),
                            failure: "container start failed".to_owned(),
                            source: Box::new(CandidateCleanupError::RemoveContainer {
                                source: podman_error(),
                            }),
                        },
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "deploy image: cleanup divergence from a lingering container",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::Cleanup {
                            deployment_id: "deployment-1".to_owned(),
                            failure: "container start failed".to_owned(),
                            source: Box::new(CandidateCleanupError::ContainerNotRemoved {
                                container_id: "abc123".to_owned(),
                            }),
                        },
                    }),
                },
                CliErrorClass::External,
            ),
            (
                "deploy image: cleanup divergence from runtime change",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::Cleanup {
                            deployment_id: "deployment-1".to_owned(),
                            failure: "container start failed".to_owned(),
                            source: Box::new(CandidateCleanupError::RuntimeChanged {
                                runtime_id: runtime_id.clone(),
                            }),
                        },
                    }),
                },
                CliErrorClass::Conflict,
            ),
            (
                "deploy image: cleanup divergence from port release",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::Cleanup {
                            deployment_id: "deployment-1".to_owned(),
                            failure: "container start failed".to_owned(),
                            source: Box::new(CandidateCleanupError::ReleasePort {
                                source: PortAllocationError::Exhausted {
                                    start: 30000,
                                    end: 39999,
                                },
                            }),
                        },
                    }),
                },
                CliErrorClass::Failure,
            ),
            (
                "deploy image: cleanup divergence from persistence",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::Cleanup {
                            deployment_id: "deployment-1".to_owned(),
                            failure: "container start failed".to_owned(),
                            source: Box::new(CandidateCleanupError::Persistence {
                                source: sqlite_error(),
                            }),
                        },
                    }),
                },
                CliErrorClass::Failure,
            ),
            (
                "deploy image: failure recording divergence from persistence",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::RecordFailure {
                            deployment_id: "deployment-1".to_owned(),
                            failure: "container start failed".to_owned(),
                            source: TransitionDeploymentError::Persistence {
                                source: sqlite_error(),
                            },
                        },
                    }),
                },
                CliErrorClass::Failure,
            ),
            (
                "deploy image: failure recording divergence from state conflict",
                CliError::DeployOci {
                    source: Box::new(DeployOciError::DeployRelease {
                        source: DeployReleaseError::RecordFailure {
                            deployment_id: "deployment-1".to_owned(),
                            failure: "container start failed".to_owned(),
                            source: TransitionDeploymentError::CannotFail {
                                deployment_id: "deployment-1".to_owned(),
                                actual: pneuma::domain::deployment::DeploymentStatus::Succeeded,
                            },
                        },
                    }),
                },
                CliErrorClass::Conflict,
            ),
        ];

        for (description, error, expected) in cases {
            assert_eq!(error.class(), expected, "{description}");
            assert_eq!(
                error.class().exit_code(),
                expected.exit_code(),
                "{description}"
            );
        }
    }

    // The full transparent wrapper chain stays intact: the original cause of a
    // nested deployment failure remains reachable through `source()`.
    #[test]
    fn nested_deployment_failures_preserve_their_source_chain() {
        let error = CliError::DeployBranch {
            source: Box::new(DeployBranchError::DeployOci {
                source: DeployOciError::DeployRelease {
                    source: DeployReleaseError::DeploymentFailed {
                        deployment_id: "deployment-1".to_owned(),
                        code: DeploymentFailureCode::RuntimeStart,
                        source: Box::new(io::Error::other("container start failed")),
                    },
                },
            }),
        };

        let mut cause = error
            .source()
            .expect("a nested deployment failure must keep its cause");
        loop {
            match cause.downcast_ref::<io::Error>() {
                Some(original) => {
                    assert_eq!(original.to_string(), "container start failed");
                    return;
                }
                None => cause = cause.source().expect("the chain must reach the cause"),
            }
        }
    }

    #[test]
    fn transparent_cli_errors_forward_their_source_chain() {
        // Transparent CLI variants forward Display and the source chain to the inner
        // use-case error, so a caused inner error surfaces through the CLI error.
        let error = CliError::Import {
            source: RemoteImportError::Workspace {
                source: io::Error::other("disk full"),
            },
        };
        assert!(error.to_string().contains("disk full"));
        let source = error
            .source()
            .expect("a caused inner error must surface its cause");
        assert!(source.downcast_ref::<io::Error>().is_some());

        let error = CliError::Rollback {
            source: RollbackError::ApplicationNotFound {
                application_id: "app-1".to_owned(),
            },
        };
        assert_eq!(error.to_string(), "application `app-1` was not found");
        assert!(error.source().is_none());
    }

    #[test]
    fn cli_query_errors_name_the_operation_and_keep_their_causes() {
        // The forwarding-only use-case wrappers were removed; the operation prefix
        // now lives here, and the store/SQLite cause must remain reachable.
        let cases = [
            (
                CliError::List {
                    source: store_error(),
                },
                "failed to list applications: application store error: Invalid parameter name: test",
            ),
            (
                CliError::ApplicationLookup {
                    source: store_error(),
                },
                "failed to load application: application store error: Invalid parameter name: test",
            ),
            (
                CliError::ListDeployments {
                    source: DeploymentStoreError::Stale {
                        deployment_id: "deployment-1".to_owned(),
                    },
                },
                "failed to list deployments: deployment `deployment-1` changed before persistence",
            ),
            (
                CliError::SystemCreate {
                    source: sqlite_error(),
                },
                "failed to create system: Invalid parameter name: test",
            ),
            (
                CliError::SystemList {
                    source: sqlite_error(),
                },
                "failed to list systems: Invalid parameter name: test",
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            let source = error
                .source()
                .expect("the original cause must stay reachable");
            assert!(!source.to_string().is_empty());
        }
    }

    #[test]
    fn deployment_failure_diagnostics_reach_the_cli_boundary() {
        use std::error::Error as _;

        use pneuma::domain::deployment::DeploymentFailureCode;
        use pneuma::domain::identity::RuntimeInstanceId;
        use pneuma::use_cases::deployment::{
            CandidateCleanupError, DeployOciError, DeployReleaseError, TransitionDeploymentError,
        };

        // Deploy failures reach the CLI wrapped by the OCI deploy operation; the
        // transparent layers must not erase diagnostics or the cause chain.
        fn deploy_failure(source: DeployReleaseError) -> CliError {
            CliError::DeployOci {
                source: Box::new(DeployOciError::DeployRelease { source }),
            }
        }

        // Application not found: nested absence classifies as missing.
        let error = deploy_failure(DeployReleaseError::ApplicationNotFound {
            application_id: "portal".to_owned(),
        });
        assert_eq!(error.class(), CliErrorClass::NotFound);
        assert!(
            error
                .to_string()
                .contains("application `portal` was not found")
        );

        // Per-Application lock contention is a caller-visible conflict.
        let error = CliError::DeployOci {
            source: Box::new(DeployOciError::ApplicationBusy {
                application_id: "portal".to_owned(),
            }),
        };
        assert_eq!(error.class(), CliErrorClass::Conflict);
        assert!(
            error
                .to_string()
                .contains("already has an operation in progress")
        );

        // Ordinary deployment failure: id, semantic code, and original cause stay visible.
        // A systemd start stage is an external integration failure.
        let error = deploy_failure(DeployReleaseError::DeploymentFailed {
            deployment_id: "deployment-1".to_owned(),
            code: DeploymentFailureCode::RuntimeStart,
            source: Box::new(io::Error::other("container start failed")),
        });
        assert_eq!(error.class(), CliErrorClass::External);
        let message = error.to_string();
        assert!(message.contains("deployment-1"), "{message}");
        assert!(message.contains("runtime_start_failed"), "{message}");
        assert!(message.contains("container start failed"), "{message}");
        let cause = error
            .source()
            .expect("a deployment failure must keep its cause");
        assert!(cause.downcast_ref::<io::Error>().is_some());

        // Cleanup divergence: the divergence is reported and the original failure
        // text is preserved alongside it; a runtime divergence is a conflict.
        let error = deploy_failure(DeployReleaseError::Cleanup {
            deployment_id: "deployment-1".to_owned(),
            failure: "container start failed".to_owned(),
            source: Box::new(CandidateCleanupError::RuntimeChanged {
                runtime_id: RuntimeInstanceId::new("11111111111111111111111111111111").unwrap(),
            }),
        });
        assert_eq!(error.class(), CliErrorClass::Conflict);
        let message = error.to_string();
        assert!(message.contains("could not be cleaned up"), "{message}");
        assert!(message.contains("container start failed"), "{message}");
        let source = error
            .source()
            .expect("a cleanup divergence must keep its cause");
        assert!(
            source
                .downcast_ref::<Box<CandidateCleanupError>>()
                .is_some_and(|error| matches!(
                    error.as_ref(),
                    CandidateCleanupError::RuntimeChanged { .. }
                ))
        );

        // Failure-recording divergence: same preservation guarantees; a missing
        // deployment row classifies as missing.
        let error = deploy_failure(DeployReleaseError::RecordFailure {
            deployment_id: "deployment-1".to_owned(),
            failure: "container start failed".to_owned(),
            source: TransitionDeploymentError::DeploymentNotFound {
                deployment_id: "deployment-1".to_owned(),
            },
        });
        assert_eq!(error.class(), CliErrorClass::NotFound);
        let message = error.to_string();
        assert!(message.contains("could not be recorded"), "{message}");
        assert!(message.contains("container start failed"), "{message}");
        let source = error
            .source()
            .expect("a recording divergence must keep its cause");
        assert!(
            source
                .downcast_ref::<TransitionDeploymentError>()
                .is_some_and(|error| matches!(
                    error,
                    TransitionDeploymentError::DeploymentNotFound { .. }
                ))
        );
    }
}
