use std::fmt;

use thiserror::Error;

use crate::domain::identity::SystemId;

#[derive(Debug, Clone, PartialEq, Eq)]
// Entity: durable organizational grouping assigned to Applications and the
// invariant authority for its catalog row (id/name/description written once at
// creation; `create_or_load` is idempotent by name).
pub struct System {
    pub id: SystemId,
    pub name: SystemName,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Catalog name of one organizational grouping. Uses the same grammar as
// `ApplicationName` (shared validator) so the catalog stays uniform.
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

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SystemName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid system name `{value}`")]
pub struct InvalidSystemName {
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::SystemName;

    #[test]
    fn accepts_catalog_names_within_the_shared_rule() {
        assert!(SystemName::new("a").is_ok());
        assert!(SystemName::new("team-system").is_ok());
        let longest_allowed = format!("a{}b", "c".repeat(61));
        assert_eq!(longest_allowed.len(), 63);
        assert!(SystemName::new(&longest_allowed).is_ok());
    }

    #[test]
    fn rejects_names_outside_the_shared_rule() {
        let too_long = format!("{}c", "a".repeat(63));
        assert_eq!(too_long.len(), 64);
        for invalid in [
            "",
            "Team",
            "team system",
            "team_system",
            "-team",
            "team-",
            "team.system",
            too_long.as_str(),
        ] {
            assert!(SystemName::new(invalid).is_err(), "{invalid:?}");
        }
    }
}
