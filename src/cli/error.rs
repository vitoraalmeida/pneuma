use thiserror::Error;

use pneuma::adapters::database::DatabaseError;
use pneuma::domain::release::InvalidOciArtifact;
use pneuma::domain::system::InvalidSystemName;
use pneuma::use_cases::application::{
    ImportError, ListError, LookupError, RemoteImportError, RuntimeLifecycleError,
};
use pneuma::use_cases::ci::CiDispatchError;
use pneuma::use_cases::deployment::{
    DeployBranchError, DeployOciError, ListDeploymentsError, RollbackError,
};
use pneuma::use_cases::exposure::ExposureChangeError;
use pneuma::use_cases::reconciliation::ReconciliationReadError;

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
    #[error(transparent)]
    List { source: ListError },
    #[error(transparent)]
    ApplicationLookup { source: LookupError },
    #[error(transparent)]
    ListDeployments { source: ListDeploymentsError },
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
    #[error(transparent)]
    DatabaseBackup { source: DatabaseError },
    #[error(transparent)]
    DatabaseRestore { source: DatabaseError },
    #[error(transparent)]
    SystemCreate {
        source: pneuma::use_cases::system::CreateError,
    },
    #[error(transparent)]
    SystemList {
        source: pneuma::use_cases::system::ListSystemsError,
    },
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
}

impl CliError {
    /// Classifies the failure for message/exit-code presentation without erasing context.
    pub(crate) fn class(&self) -> CliErrorClass {
        match self {
            Self::InvalidOciArtifact { .. } | Self::MissingDeployOption => CliErrorClass::Usage,
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
                _ => CliErrorClass::Failure,
            },
            Self::CiDispatch { .. } => CliErrorClass::Usage,
            Self::Reconcile { source } => classify_reconciliation_read(source),
            Self::Database { .. }
            | Self::List { .. }
            | Self::ApplicationLookup { .. }
            | Self::ListDeployments { .. }
            | Self::DatabaseBackup { .. }
            | Self::DatabaseRestore { .. }
            | Self::SystemCreate { .. }
            | Self::SystemList { .. }
            | Self::Doctor => CliErrorClass::Failure,
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
            ImportError::Manifest { .. } => CliErrorClass::Usage,
            _ => CliErrorClass::Failure,
        },
    }
}

fn classify_runtime_lifecycle(source: &RuntimeLifecycleError) -> CliErrorClass {
    match source {
        RuntimeLifecycleError::NotDeployed { .. }
        | RuntimeLifecycleError::ContainerMissing { .. } => CliErrorClass::NotFound,
        RuntimeLifecycleError::RuntimeChanged { .. } => CliErrorClass::Conflict,
        RuntimeLifecycleError::Observe { .. }
        | RuntimeLifecycleError::Control { .. }
        | RuntimeLifecycleError::Supervision { .. } => CliErrorClass::External,
        _ => CliErrorClass::Failure,
    }
}

fn classify_deploy_oci(source: &DeployOciError) -> CliErrorClass {
    match source {
        DeployOciError::RepositoryMismatch { .. } => CliErrorClass::Usage,
        DeployOciError::PullImage { .. } => CliErrorClass::External,
        _ => CliErrorClass::Failure,
    }
}

fn classify_deploy_branch(source: &DeployBranchError) -> CliErrorClass {
    match source {
        DeployBranchError::ResolveBranch { .. } | DeployBranchError::ResolveImageDigest { .. } => {
            CliErrorClass::External
        }
        DeployBranchError::DeployOci { source } => classify_deploy_oci(source),
        _ => CliErrorClass::Failure,
    }
}

fn classify_rollback(source: &RollbackError) -> CliErrorClass {
    match source {
        RollbackError::ApplicationNotFound { .. } => CliErrorClass::NotFound,
        RollbackError::NoPreviousDeployment { .. } => CliErrorClass::Conflict,
        RollbackError::PullImage { .. } => CliErrorClass::External,
        _ => CliErrorClass::Failure,
    }
}

fn classify_exposure_change(source: &ExposureChangeError) -> CliErrorClass {
    match source {
        ExposureChangeError::ApplicationNotFound { .. }
        | ExposureChangeError::NoActiveRuntime { .. } => CliErrorClass::NotFound,
        ExposureChangeError::ExposureChanged { .. }
        | ExposureChangeError::RuntimeNotRunning { .. } => CliErrorClass::Conflict,
        ExposureChangeError::InvalidVisibility { .. } => CliErrorClass::Usage,
        ExposureChangeError::ObserveFailed { .. }
        | ExposureChangeError::MaterializeFailed { .. }
        | ExposureChangeError::RemoveFragmentFailed { .. }
        | ExposureChangeError::ExternalHealthFailed { .. } => CliErrorClass::External,
        _ => CliErrorClass::Failure,
    }
}

fn classify_reconciliation_read(source: &ReconciliationReadError) -> CliErrorClass {
    match source {
        ReconciliationReadError::ApplicationNotFound { .. } => CliErrorClass::NotFound,
        ReconciliationReadError::OperationLock { .. } => CliErrorClass::Conflict,
        ReconciliationReadError::ObserveContainer { .. }
        | ReconciliationReadError::ObserveNamedContainer { .. }
        | ReconciliationReadError::ObserveQuadlet { .. }
        | ReconciliationReadError::ObserveCaddy { .. } => CliErrorClass::External,
        _ => CliErrorClass::Failure,
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    fn assert_class(error: CliError, expected: CliErrorClass) {
        assert_eq!(error.class(), expected);
    }

    #[test]
    fn exit_codes_are_stable_per_class() {
        assert_eq!(CliErrorClass::Failure.exit_code(), 1);
        assert_eq!(CliErrorClass::Usage.exit_code(), 2);
        assert_eq!(CliErrorClass::NotFound.exit_code(), 3);
        assert_eq!(CliErrorClass::Conflict.exit_code(), 4);
        assert_eq!(CliErrorClass::External.exit_code(), 5);
    }

    #[test]
    fn classifies_rejected_command_input_as_usage() {
        let artifact = pneuma::domain::release::OciArtifact::parse("not-a-digest")
            .expect_err("invalid reference must be rejected");
        assert_class(
            CliError::InvalidOciArtifact { source: artifact },
            CliErrorClass::Usage,
        );
        assert_class(CliError::MissingDeployOption, CliErrorClass::Usage);
        assert_class(
            CliError::CiDispatch {
                source: pneuma::use_cases::ci::CiDispatchError::EmptyCommand,
            },
            CliErrorClass::Usage,
        );
    }

    #[test]
    fn classifies_absent_named_resources_as_not_found() {
        assert_class(
            CliError::ApplicationNotFound {
                application_name: "portal".to_owned(),
            },
            CliErrorClass::NotFound,
        );
        assert_class(
            CliError::Reconcile {
                source: ReconciliationReadError::ApplicationNotFound {
                    application_name: "portal".to_owned(),
                },
            },
            CliErrorClass::NotFound,
        );
        assert_class(
            CliError::ApplicationRuntime {
                source: Box::new(RuntimeLifecycleError::NotDeployed {
                    application_name: "portal".to_owned(),
                }),
            },
            CliErrorClass::NotFound,
        );
        assert_class(
            CliError::SystemShow {
                source: pneuma::use_cases::system::ShowError::NotFound {
                    system_name: "billing".to_owned(),
                },
            },
            CliErrorClass::NotFound,
        );
    }

    #[test]
    fn classifies_unsatisfied_or_concurrent_state_as_conflict() {
        assert_class(
            CliError::ApplicationRuntime {
                source: Box::new(RuntimeLifecycleError::RuntimeChanged {
                    runtime_id: "runtime-1".to_owned(),
                }),
            },
            CliErrorClass::Conflict,
        );
        assert_class(
            CliError::VisibilitySet {
                source: ExposureChangeError::ExposureChanged {
                    application_id: "app-1".to_owned(),
                },
            },
            CliErrorClass::Conflict,
        );
        assert_class(
            CliError::Rollback {
                source: RollbackError::NoPreviousDeployment {
                    application_id: "app-1".to_owned(),
                },
            },
            CliErrorClass::Conflict,
        );
        assert_class(
            CliError::Reconcile {
                source: ReconciliationReadError::OperationLock {
                    source: pneuma::adapters::application_lock::ApplicationLockError::Open {
                        path: "/tmp/lock".into(),
                        source: io::Error::other("locked"),
                    },
                },
            },
            CliErrorClass::Conflict,
        );
    }

    #[test]
    fn classifies_external_integration_failures_as_external() {
        assert_class(
            CliError::DeployOci {
                source: Box::new(DeployOciError::PullImage {
                    source: pneuma::adapters::oci_image::PullImageError::Pull {
                        reference: "registry.example/app@sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
                        stdout: String::new(),
                        stderr: "denied".to_owned(),
                    },
                }),
            },
            CliErrorClass::External,
        );
        assert_class(
            CliError::DeployBranch {
                source: Box::new(DeployBranchError::ResolveBranch {
                    source: pneuma::adapters::git_source::ResolveBranchError::BranchNotFound {
                        url: "https://git.example/app.git".to_owned(),
                        branch: "main".to_owned(),
                    },
                }),
            },
            CliErrorClass::External,
        );
        assert_class(
            CliError::Import {
                source: RemoteImportError::Clone {
                    source: clone_error_for_test(),
                },
            },
            CliErrorClass::External,
        );
    }

    fn clone_error_for_test() -> pneuma::adapters::git_source::CloneRepositoryError {
        pneuma::adapters::git_source::CloneRepositoryError::Execute {
            operation: "clone",
            source: io::Error::other("no network"),
        }
    }

    #[test]
    fn classifies_persistence_and_internal_failures_as_failure() {
        assert_class(CliError::Doctor, CliErrorClass::Failure);
        assert_class(
            CliError::SystemCreate {
                source: pneuma::use_cases::system::CreateError::Persistence {
                    source: rusqlite::Error::InvalidParameterName("test".to_owned()),
                },
            },
            CliErrorClass::Failure,
        );
        assert_class(
            CliError::Import {
                source: RemoteImportError::Workspace {
                    source: io::Error::other("disk full"),
                },
            },
            CliErrorClass::Failure,
        );
        assert_class(
            CliError::Import {
                source: RemoteImportError::InvalidRepository,
            },
            CliErrorClass::Usage,
        );
    }
}
