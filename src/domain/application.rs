#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub id: String,
    pub system_id: String,
    pub name: String,
    pub repository: String,
    pub default_branch: String,
    pub active_deployment_id: Option<String>,
}
