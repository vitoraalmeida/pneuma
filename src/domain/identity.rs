use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SystemId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ApplicationId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReleaseId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeploymentId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RuntimeInstanceId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContainerId(String);

impl SystemId {
    // Preserves legacy SQLite text without imposing a new identifier format.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ApplicationId {
    // Preserves legacy SQLite text without imposing a new identifier format.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ReleaseId {
    // Preserves legacy SQLite text without imposing a new identifier format.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl DeploymentId {
    // Preserves legacy SQLite text without imposing a new identifier format.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RuntimeInstanceId {
    // Preserves legacy SQLite text without imposing a new identifier format.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ContainerId {
    // Preserves adapter-provided container text separately from logical IDs.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SystemId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SystemId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for SystemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for ApplicationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ApplicationId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for ApplicationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for ReleaseId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ReleaseId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for ReleaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for DeploymentId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for DeploymentId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for DeploymentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for RuntimeInstanceId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for RuntimeInstanceId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for RuntimeInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for ContainerId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ContainerId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for ContainerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{ApplicationId, ContainerId, DeploymentId};

    #[test]
    fn preserves_legacy_text_without_a_format_requirement() {
        let application_id = ApplicationId::from("legacy value/with punctuation");

        assert_eq!(application_id.as_str(), "legacy value/with punctuation");
        assert_eq!(application_id.to_string(), "legacy value/with punctuation");
    }

    #[test]
    fn logical_and_external_id_apis_are_not_interchangeable() {
        fn deployment_for(_application_id: ApplicationId, _deployment_id: DeploymentId) {}
        fn observe_container(_container_id: ContainerId) {}

        deployment_for(
            ApplicationId::from("application"),
            DeploymentId::from("deployment"),
        );
        observe_container(ContainerId::from("container"));
    }
}
