use std::fmt;

use thiserror::Error;

// One distinct nominal type per entity so an `ApplicationId` can never be
// passed where a `DeploymentId` is required, even though all of them are
// SQLite text under the hood. Every identifier validates the current
// store-generated format (32 lowercase hexadecimal characters from
// `lower(hex(randomblob(16)))`) at construction, so legacy or malformed text
// can never become an identifier; hydration and generation go through the
// same constructor.
//
// All identifiers share the same mechanical shape, so a private local macro
// stamps out the identical tuple struct, validated constructor, `as_str`, and
// `Display` impls while each invocation still declares a genuinely distinct
// type. The only per-type difference today is the visibility of `as_str`:
// types whose inner text integration tests read directly expose it as `pub`,
// while internal-only identifiers keep it crate-private. Any future
// identifier with its own validation or semantics must stay out of this
// macro and be written by hand.
macro_rules! identity_newtype {
    ($name:ident, $as_str_visibility:vis) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            // Accepts only the current store-generated identifier format.
            pub fn new(value: &str) -> Result<Self, InvalidEntityId> {
                if !is_valid_entity_id(value) {
                    return Err(InvalidEntityId {
                        value: value.to_owned(),
                    });
                }
                Ok(Self(value.to_owned()))
            }

            $as_str_visibility fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

identity_newtype!(SystemId, pub(crate));
identity_newtype!(ApplicationId, pub);
identity_newtype!(ReleaseId, pub);
identity_newtype!(DeploymentId, pub);
identity_newtype!(RuntimeInstanceId, pub);

// Lets the persistence layer validate hydrated identifier text through one
// constructor without duplicating per-type mapping code in every store.
pub(crate) trait EntityId: Sized {
    const FIELD_NAME: &'static str;
    fn parse(value: &str) -> Result<Self, InvalidEntityId>;
}

const ENTITY_ID_LENGTH: usize = 32;

// The exact shape produced by `lower(hex(randomblob(16)))` in every store.
fn is_valid_entity_id(value: &str) -> bool {
    value.len() == ENTITY_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[derive(Debug, PartialEq, Eq, Error)]
#[error("invalid entity identifier `{value}`: expected 32 lowercase hexadecimal characters")]
pub struct InvalidEntityId {
    pub value: String,
}

impl EntityId for SystemId {
    const FIELD_NAME: &'static str = "system id";
    fn parse(value: &str) -> Result<Self, InvalidEntityId> {
        Self::new(value)
    }
}

impl EntityId for ApplicationId {
    const FIELD_NAME: &'static str = "application id";
    fn parse(value: &str) -> Result<Self, InvalidEntityId> {
        Self::new(value)
    }
}

impl EntityId for ReleaseId {
    const FIELD_NAME: &'static str = "release id";
    fn parse(value: &str) -> Result<Self, InvalidEntityId> {
        Self::new(value)
    }
}

impl EntityId for DeploymentId {
    const FIELD_NAME: &'static str = "deployment id";
    fn parse(value: &str) -> Result<Self, InvalidEntityId> {
        Self::new(value)
    }
}

impl EntityId for RuntimeInstanceId {
    const FIELD_NAME: &'static str = "runtime id";
    fn parse(value: &str) -> Result<Self, InvalidEntityId> {
        Self::new(value)
    }
}

// Single authority for the Application/System name grammar so both entities
// (and the SSH dispatcher, which must reject exactly what the catalog rejects)
// share one rule instead of three drifted copies.
pub(crate) fn is_valid_catalog_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::{
        ApplicationId, DeploymentId, ReleaseId, RuntimeInstanceId, SystemId, is_valid_catalog_name,
    };

    fn valid_id(seed: u8) -> String {
        format!("{seed:032x}")
    }

    #[test]
    fn accepts_the_current_store_generated_format() {
        for value in [valid_id(0), valid_id(255), "a".repeat(32)] {
            assert!(ApplicationId::new(&value).is_ok(), "{value:?}");
        }
    }

    #[test]
    fn rejects_text_outside_the_current_identifier_format() {
        for value in [
            "",
            "app",
            "legacy value/with punctuation",
            &"A".repeat(32),
            &format!("{}g", "a".repeat(31)),
            &"a".repeat(31),
            &"a".repeat(33),
        ] {
            let error = ApplicationId::new(value).unwrap_err();
            assert_eq!(error.value, value);
            assert_eq!(
                error.to_string(),
                format!(
                    "invalid entity identifier `{value}`: expected 32 lowercase hexadecimal characters"
                )
            );
        }
    }

    #[test]
    fn every_identifier_kind_validates_the_same_format() {
        for value in ["app", "sys 001", "rel#7", "dep 42", "runtime_1"] {
            assert!(SystemId::new(value).is_err(), "{value:?}");
            assert!(ReleaseId::new(value).is_err(), "{value:?}");
            assert!(DeploymentId::new(value).is_err(), "{value:?}");
            assert!(RuntimeInstanceId::new(value).is_err(), "{value:?}");
        }
        let value = valid_id(1);
        assert!(SystemId::new(&value).is_ok());
        assert!(ReleaseId::new(&value).is_ok());
        assert!(DeploymentId::new(&value).is_ok());
        assert!(RuntimeInstanceId::new(&value).is_ok());
    }

    #[test]
    fn identifiers_display_their_validated_text() {
        let value = valid_id(2);
        let application_id = ApplicationId::new(&value).unwrap();
        assert_eq!(application_id.as_str(), value);
        assert_eq!(application_id.to_string(), value);
    }

    #[test]
    fn catalog_names_are_lowercase_alphanumeric_with_inner_hyphens() {
        let longest_allowed = format!("a{}b", "c".repeat(61));
        assert_eq!(longest_allowed.len(), 63);
        for valid in ["a", "team-system", longest_allowed.as_str()] {
            assert!(is_valid_catalog_name(valid), "{valid:?}");
        }

        let too_long = format!("{longest_allowed}c");
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
            assert!(!is_valid_catalog_name(invalid), "{invalid:?}");
        }
    }
}
