use std::error::Error;
use std::fmt;

use pneuma::adapters::database::DatabaseError;
use pneuma::domain::release::InvalidOciArtifact;
use pneuma::domain::system::InvalidSystemName;
use pneuma::use_cases::application::{
    ListError, LookupError, RemoteImportError, RuntimeLifecycleError,
};
use pneuma::use_cases::ci::CiDispatchError;
use pneuma::use_cases::deployment::{
    DeployBranchError, DeployOciError, ListDeploymentsError, RollbackError,
};
use pneuma::use_cases::exposure::ExposureChangeError;
use pneuma::use_cases::reconciliation::ReconciliationReadError;

#[derive(Debug)]
pub(crate) enum CliError {
    Database {
        source: DatabaseError,
    },
    Import {
        source: RemoteImportError,
    },
    InvalidSystemName {
        source: InvalidSystemName,
    },
    List {
        source: ListError,
    },
    ApplicationLookup {
        source: LookupError,
    },
    ListDeployments {
        source: ListDeploymentsError,
    },
    ApplicationNotFound {
        application_name: String,
    },
    ApplicationRuntime {
        source: Box<RuntimeLifecycleError>,
    },
    DeployOci {
        source: Box<DeployOciError>,
    },
    InvalidOciArtifact {
        source: InvalidOciArtifact,
    },
    DeployBranch {
        source: Box<DeployBranchError>,
    },
    Rollback {
        source: RollbackError,
    },
    VisibilitySet {
        source: ExposureChangeError,
    },
    DatabaseBackup {
        source: DatabaseError,
    },
    DatabaseRestore {
        source: DatabaseError,
    },
    SystemCreate {
        source: pneuma::use_cases::system::CreateError,
    },
    SystemList {
        source: pneuma::use_cases::system::ListSystemsError,
    },
    SystemShow {
        source: pneuma::use_cases::system::ShowError,
    },
    CiDispatch {
        source: CiDispatchError,
    },
    Reconcile {
        source: ReconciliationReadError,
    },
    Doctor,
    MissingDeployOption,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database { source } => write!(formatter, "{source}"),
            Self::Import { source } => write!(formatter, "{source}"),
            Self::InvalidSystemName { source } => write!(formatter, "{source}"),
            Self::List { source } => write!(formatter, "{source}"),
            Self::ApplicationLookup { source } => write!(formatter, "{source}"),
            Self::ListDeployments { source } => write!(formatter, "{source}"),
            Self::ApplicationNotFound { application_name } => {
                write!(formatter, "application `{application_name}` was not found")
            }
            Self::ApplicationRuntime { source } => write!(formatter, "{source}"),
            Self::DeployOci { source } => write!(formatter, "{source}"),
            Self::InvalidOciArtifact { source } => write!(formatter, "{source}"),
            Self::DeployBranch { source } => write!(formatter, "{source}"),
            Self::Rollback { source } => write!(formatter, "{source}"),
            Self::VisibilitySet { source } => write!(formatter, "{source}"),
            Self::DatabaseBackup { source } | Self::DatabaseRestore { source } => {
                write!(formatter, "{source}")
            }
            Self::SystemCreate { source } => write!(formatter, "{source}"),
            Self::SystemList { source } => write!(formatter, "{source}"),
            Self::SystemShow { source } => write!(formatter, "{source}"),
            Self::CiDispatch { source } => write!(formatter, "{source}"),
            Self::Reconcile { source } => write!(formatter, "{source}"),
            Self::Doctor => formatter.write_str("one or more diagnostic checks failed"),
            Self::MissingDeployOption => {
                formatter.write_str("either --image or --branch must be specified")
            }
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database { source } => Some(source),
            Self::Import { source } => Some(source),
            Self::InvalidSystemName { source } => Some(source),
            Self::List { source } => Some(source),
            Self::ApplicationLookup { source } => Some(source),
            Self::ListDeployments { source } => Some(source),
            Self::DeployOci { source } => Some(source.as_ref()),
            Self::InvalidOciArtifact { source } => Some(source),
            Self::DeployBranch { source } => Some(source.as_ref()),
            Self::ApplicationRuntime { source } => Some(source.as_ref()),
            Self::ApplicationNotFound { .. } => None,
            Self::Rollback { source } => Some(source),
            Self::VisibilitySet { source } => Some(source),
            Self::DatabaseBackup { source } | Self::DatabaseRestore { source } => Some(source),
            Self::SystemCreate { source } => Some(source),
            Self::SystemList { source } => Some(source),
            Self::SystemShow { source } => Some(source),
            Self::CiDispatch { source } => Some(source),
            Self::Reconcile { source } => Some(source),
            Self::Doctor | Self::MissingDeployOption => None,
        }
    }
}
