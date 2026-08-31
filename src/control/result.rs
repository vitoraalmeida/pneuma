use crate::domain::system::System;
use crate::use_cases::system::SystemDetails;

/// Typed result of one executed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    SystemCreated(System),
    Systems(Vec<System>),
    SystemDetails(SystemDetails),
}
