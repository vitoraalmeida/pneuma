use std::fmt;

// One distinct nominal type per entity so an `ApplicationId` can never be
// passed where a `DeploymentId` is required, even though all of them are
// SQLite text under the hood. They are deliberately non-validating: rows
// persisted before this type system existed carry arbitrary legacy text
// (INV-DB-006), so imposing a format here would break hydration of real data.
// Construction is unrestricted (`From`) because only stores mint identifiers;
// everywhere else they flow in as already-persisted facts.
//
// All identifiers share the same mechanical shape, so a private local macro
// stamps out the identical tuple struct, `as_str`, `From`, and `Display`
// impls while each invocation still declares a genuinely distinct type.
// The only per-type difference today is the visibility of `as_str`: types
// whose inner text integration tests read directly expose it as `pub`,
// while internal-only identifiers keep it crate-private. Any future
// identifier with its own validation or semantics must stay out of this
// macro and be written by hand.
macro_rules! identity_newtype {
    ($name:ident, $as_str_visibility:vis) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            // Preserves legacy SQLite text without imposing a new identifier format.
            $as_str_visibility fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
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

    #[test]
    fn preserves_legacy_text_without_a_format_requirement() {
        let application_id = ApplicationId::from("legacy value/with punctuation");

        assert_eq!(application_id.as_str(), "legacy value/with punctuation");
        assert_eq!(application_id.to_string(), "legacy value/with punctuation");
    }

    #[test]
    fn every_identifier_kind_preserves_arbitrary_legacy_text() {
        assert_eq!(SystemId::from("sys 001").to_string(), "sys 001");
        assert_eq!(
            ApplicationId::from("app/legacy id").to_string(),
            "app/legacy id"
        );
        assert_eq!(ReleaseId::from("rel#7").to_string(), "rel#7");
        assert_eq!(DeploymentId::from("dep 42").to_string(), "dep 42");
        assert_eq!(
            RuntimeInstanceId::from("runtime_1").to_string(),
            "runtime_1"
        );
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
