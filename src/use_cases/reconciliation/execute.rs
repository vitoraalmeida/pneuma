use rusqlite::Connection;

use crate::domain::reconciliation::{
    ReconciliationDecision, ReconciliationDecisionError, ReconciliationExpectations,
    ReconciliationInput,
};

use super::exposure_effects::{
    materialize_public_route, record_public_exposure_failure, remove_internal_route,
};
use super::runtime_effects::{confirm_runtime_identity, rematerialize_runtime};
use super::{ReconciliationReadError, ReconciliationResult};

// Translates a pure decision refusal into the read error surface without changing its message.
pub(crate) fn reconciliation_decision_reason(error: ReconciliationDecisionError) -> String {
    match error {
        ReconciliationDecisionError::UnhandledDrift => {
            "drift has no automatic repair; manual intervention is required".to_owned()
        }
        ReconciliationDecisionError::InvalidRouteFragment(source) => source.to_string(),
    }
}

// Executes one decided action; every effect corresponds to exactly one decision variant.
//
// Runtime effects (identity repair, rematerialization) live in `runtime_effects`;
// exposure effects (route removal, public materialization, failure recording) live
// in `exposure_effects`. This dispatcher owns only the decision-to-effect mapping.
pub(crate) fn execute_reconciliation_decision(
    connection: &mut Connection,
    input: &ReconciliationInput,
    expectations: &ReconciliationExpectations,
    decision: ReconciliationDecision,
    managed_caddy_directory: &std::path::Path,
    caddyfile_path: &std::path::Path,
) -> Result<ReconciliationResult, ReconciliationReadError> {
    match decision {
        ReconciliationDecision::InSync => Ok(ReconciliationResult::NoOp),
        ReconciliationDecision::RepairRuntime(repair) => {
            confirm_runtime_identity(connection, input, &repair)
        }
        ReconciliationDecision::RematerializeRuntime(plan) => {
            rematerialize_runtime(connection, input, expectations, plan)
        }
        ReconciliationDecision::RemoveInternalRoute { expected_state } => remove_internal_route(
            connection,
            input,
            expected_state,
            managed_caddy_directory,
            caddyfile_path,
        ),
        ReconciliationDecision::MaterializePublicRoute { expected_state } => {
            materialize_public_route(
                connection,
                input,
                expected_state,
                managed_caddy_directory,
                caddyfile_path,
            )
        }
        ReconciliationDecision::RecordPublicExposureFailure(failure) => {
            record_public_exposure_failure(connection, input, &failure)
        }
        ReconciliationDecision::RequireManualIntervention(reason) => {
            Ok(ReconciliationResult::ManualIntervention { reason })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unhandled_drift_reason_names_the_missing_automatic_repair() {
        let reason = reconciliation_decision_reason(ReconciliationDecisionError::UnhandledDrift);
        assert_eq!(
            reason,
            "drift has no automatic repair; manual intervention is required"
        );
    }

    #[test]
    fn invalid_route_fragment_reason_preserves_the_source_message() {
        let error = ReconciliationDecisionError::InvalidRouteFragment(
            crate::domain::exposure::InvalidExposureConfigurationVersion {
                value: "<invalid>".to_owned(),
            },
        );
        let reason = reconciliation_decision_reason(error);
        assert_eq!(reason, "invalid exposure configuration version `<invalid>`");
    }
}
