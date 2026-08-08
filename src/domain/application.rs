#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub id: String,
    pub system_id: Option<String>,
    pub name: String,
    pub repository: Option<String>,
    pub default_branch: Option<String>,
    pub active_deployment_id: Option<String>,
}
