use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
// Represents the immutable full commit identifier shared by Git, OCI tags, and Deployments.
pub struct CommitSha(String);

impl CommitSha {
    pub fn new(value: &str) -> Result<Self, InvalidCommitSha> {
        if value.len() != 40
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(InvalidCommitSha {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidCommitSha {
    pub value: String,
}

impl fmt::Display for InvalidCommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid commit SHA `{}`", self.value)
    }
}

impl Error for InvalidCommitSha {}
