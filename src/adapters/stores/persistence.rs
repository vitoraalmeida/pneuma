use crate::adapters::stores::PersistenceOutcome;
use crate::domain::exposure::Visibility;
use crate::domain::runtime::{ObservedRuntimeState, RuntimeState};
use std::io;

// Distinguishes a confirmed compare-and-set write from a concurrent persisted change.
pub(crate) fn outcome(rows_updated: usize) -> PersistenceOutcome {
    if rows_updated == 1 {
        PersistenceOutcome::Updated
    } else {
        PersistenceOutcome::Stale
    }
}

// Converts an invalid persisted text value into a row-mapping error with column context.
pub(crate) fn invalid_text_value(column: usize, field: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {field}: {value}"),
        )),
    )
}

pub(crate) fn visibility_value(value: Visibility) -> &'static str {
    match value {
        Visibility::Internal => "internal",
        Visibility::Public => "public",
    }
}

pub(crate) fn visibility_from_value(value: &str) -> Option<Visibility> {
    match value {
        "internal" => Some(Visibility::Internal),
        "public" => Some(Visibility::Public),
        _ => None,
    }
}

pub(crate) fn runtime_state_value(value: RuntimeState) -> &'static str {
    match value {
        RuntimeState::Starting => "starting",
        RuntimeState::Running => "running",
        RuntimeState::Stopped => "stopped",
        RuntimeState::Failed => "failed",
    }
}

pub(crate) fn runtime_state_from_value(value: &str) -> Option<RuntimeState> {
    match value {
        "starting" => Some(RuntimeState::Starting),
        "running" => Some(RuntimeState::Running),
        "stopped" => Some(RuntimeState::Stopped),
        "failed" => Some(RuntimeState::Failed),
        _ => None,
    }
}

pub(crate) fn observed_runtime_state_value(value: &ObservedRuntimeState) -> &'static str {
    match value {
        ObservedRuntimeState::Missing => "missing",
        ObservedRuntimeState::Created => "created",
        ObservedRuntimeState::Starting => "starting",
        ObservedRuntimeState::Running => "running",
        ObservedRuntimeState::Stopping => "stopping",
        ObservedRuntimeState::Stopped => "stopped",
        ObservedRuntimeState::Failed => "failed",
        ObservedRuntimeState::Unknown { .. } => "unknown",
    }
}

pub(crate) fn observed_runtime_state_from_value(value: &str) -> ObservedRuntimeState {
    match value {
        "missing" => ObservedRuntimeState::Missing,
        "created" => ObservedRuntimeState::Created,
        "starting" => ObservedRuntimeState::Starting,
        "running" => ObservedRuntimeState::Running,
        "stopping" => ObservedRuntimeState::Stopping,
        "stopped" => ObservedRuntimeState::Stopped,
        "failed" => ObservedRuntimeState::Failed,
        status => ObservedRuntimeState::Unknown {
            status: status.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_outcome_is_stale_for_every_non_single_row_update() {
        assert_eq!(outcome(0), PersistenceOutcome::Stale);
        assert_eq!(outcome(1), PersistenceOutcome::Updated);
        assert_eq!(outcome(2), PersistenceOutcome::Stale);
    }

    #[test]
    fn visibility_values_round_trip_and_reject_unknown_text() {
        for value in [Visibility::Internal, Visibility::Public] {
            assert_eq!(visibility_from_value(visibility_value(value)), Some(value));
        }
        assert_eq!(visibility_from_value("unknown"), None);
    }

    #[test]
    fn runtime_state_values_round_trip_and_reject_unknown_text() {
        for value in [
            RuntimeState::Starting,
            RuntimeState::Running,
            RuntimeState::Stopped,
            RuntimeState::Failed,
        ] {
            assert_eq!(
                runtime_state_from_value(runtime_state_value(value)),
                Some(value)
            );
        }
        assert_eq!(runtime_state_from_value("removed"), None);
    }

    #[test]
    fn observed_runtime_states_round_trip_with_unknown_tolerated_as_unknown() {
        for value in [
            ObservedRuntimeState::Missing,
            ObservedRuntimeState::Created,
            ObservedRuntimeState::Starting,
            ObservedRuntimeState::Running,
            ObservedRuntimeState::Stopping,
            ObservedRuntimeState::Stopped,
            ObservedRuntimeState::Failed,
        ] {
            assert_eq!(
                observed_runtime_state_from_value(observed_runtime_state_value(&value)),
                value
            );
        }
        assert_eq!(
            observed_runtime_state_from_value("anything else"),
            ObservedRuntimeState::Unknown {
                status: "anything else".to_owned()
            }
        );
    }

    #[test]
    fn invalid_text_values_become_column_scoped_conversion_failures() {
        let error = invalid_text_value(4, "desired runtime state", "nonsense");
        match error {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                source,
            ) if index == 4 && source.to_string().contains("nonsense") => {}
            other => panic!("unexpected conversion error: {other:?}"),
        }
    }
}
