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
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 63
            || !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        {
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
