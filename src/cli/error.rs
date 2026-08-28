use thiserror::Error;

use pneuma::adapters::database::DatabaseError;
use pneuma::adapters::stores::application_store::ApplicationStoreError;
use pneuma::adapters::stores::deployment_store::DeploymentStoreError;
use pneuma::domain::release::InvalidOciArtifact;
use pneuma::domain::system::InvalidSystemName;
use pneuma::use_cases::application::{ImportError, RemoteImportError, RuntimeLifecycleError};
use pneuma::use_cases::ci::CiDispatchError;
use pneuma::use_cases::deployment::{DeployBranchError, DeployOciError, RollbackError};
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

    fn sqlite_error() -> rusqlite::Error {
        rusqlite::Error::InvalidParameterName("test".to_owned())
    }

    fn clone_error() -> CloneRepositoryError {
        CloneRepositoryError::Execute {
            operation: "clone",
            source: io::Error::other("no network"),
        }
    }

    fn pull_image_error() -> PullImageError {
        PullImageError::Pull {
            reference: "registry.example/app@sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
            stdout: String::new(),
            stderr: "denied".to_owned(),
        }
    }

    fn podman_error() -> PodmanError {
        PodmanError::Execute {
            operation: "observing",
            source: io::Error::other("no podman"),
        }
    }

    fn store_error() -> ApplicationStoreError {
        ApplicationStoreError::Persistence {
            source: sqlite_error(),
        }
    }

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
                CliErrorClass::Failure,
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
            // Branch deployment family, including nested OCI layering.
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
                "visibility: rejected visibility input",
                CliError::VisibilitySet {
                    source: ExposureChangeError::InvalidVisibility {
                        visibility: "maybe".to_owned(),
                    },
                },
                CliErrorClass::Usage,
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
                "reconcile: operation lock failure",
                CliError::Reconcile {
                    source: ReconciliationReadError::OperationLock {
                        source: pneuma::adapters::application_lock::ApplicationLockError::Open {
                            path: "/tmp/lock".into(),
                            source: io::Error::other("locked"),
                        },
                    },
                },
                CliErrorClass::Conflict,
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

    #[test]
    fn exit_codes_are_stable_per_class() {
        assert_eq!(CliErrorClass::Failure.exit_code(), 1);
        assert_eq!(CliErrorClass::Usage.exit_code(), 2);
        assert_eq!(CliErrorClass::NotFound.exit_code(), 3);
        assert_eq!(CliErrorClass::Conflict.exit_code(), 4);
        assert_eq!(CliErrorClass::External.exit_code(), 5);
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
}
