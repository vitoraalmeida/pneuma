#[derive(Debug, Clone, PartialEq, Eq)]
// Represents the durable organizational grouping assigned to Applications.
pub struct System {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}
