use std::error::Error;
use std::fmt;

use crate::domain::identity::SystemId;

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents the durable organizational grouping assigned to Applications.
pub struct System {
    pub id: SystemId,
    pub name: SystemName,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemName(String);

impl SystemName {
    pub fn new(value: &str) -> Result<Self, InvalidSystemName> {
        if !crate::domain::identity::is_valid_catalog_name(value) {
            return Err(InvalidSystemName {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SystemName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidSystemName {
    pub value: String,
}

impl fmt::Display for InvalidSystemName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid system name `{}`", self.value)
    }
}

impl Error for InvalidSystemName {}
