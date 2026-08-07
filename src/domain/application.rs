// An `Application` only exists after registration. A separate registration
// state would have a single valid value until another state is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub repository: String,
    pub default_branch: String,
}
