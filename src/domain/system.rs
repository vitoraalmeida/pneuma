use crate::domain::application::SystemName;
use crate::domain::identity::SystemId;

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents the durable organizational grouping assigned to Applications.
pub struct System {
    pub id: SystemId,
    pub name: SystemName,
    pub description: Option<String>,
}
