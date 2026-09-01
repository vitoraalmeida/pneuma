/// Interface-neutral command vocabulary. The CLI maps parsed arguments onto
/// these commands; later adapters issue the same commands without Clap.
use crate::domain::exposure::Visibility;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SystemCreate {
        name: String,
        description: Option<String>,
    },
    SystemList,
    SystemShow {
        name: String,
    },
    ImportApplication {
        repository: String,
        system_name: Option<String>,
        manifest_path: Option<String>,
    },
    ListApplications,
    ListDeployments {
        application_name: String,
    },
    ApplicationStatus {
        application_name: String,
    },
    ApplicationStop {
        application_name: String,
    },
    ApplicationStart {
        application_name: String,
    },
    VisibilitySet {
        application_name: String,
        visibility: Visibility,
    },
    Reconcile {
        application_name: String,
    },
}
