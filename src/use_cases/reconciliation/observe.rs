use std::path::Path;

use crate::adapters::caddy_exposure::{canonical_fragment_contents, observe_caddy_fragment};
use crate::adapters::local_runtime::{observe_container, observe_named_container};
use crate::adapters::systemd_quadlet::{
    canonical_unit_contents, container_name, observe_generated_unit, observe_unit_source, unit_name,
};
use crate::domain::exposure::{Exposure, ExposureIntent};
use crate::domain::reconciliation::{
    ReconciliationExpectations, ReconciliationInput, ReconciliationObservation,
};

use super::{ReconciliationReadError, host_port, required_active_runtime};

// Renders the boundary representations the pure decision compares observations against.
pub(crate) fn reconciliation_expectations(
    input: &ReconciliationInput,
) -> Result<ReconciliationExpectations, ReconciliationReadError> {
    let application = &input.desired.application;
    let (active, runtime) = required_active_runtime(input)?;
    Ok(ReconciliationExpectations {
        container_name: container_name(&application.name, &active.deployment.id),
        canonical_quadlet_contents: canonical_unit_contents(
            &application.name,
            &active.deployment.id,
            &active.release.artifact,
            runtime.container_port,
            host_port(runtime)?,
        ),
        canonical_route_fragment: match input.desired.exposure.as_ref().map(Exposure::intent) {
            Some(ExposureIntent::Public { domain }) => Some(canonical_fragment_contents(
                domain,
                runtime.expected_endpoint,
            )),
            _ => None,
        },
    })
}

// Observes persisted runtime and route identities without changing SQLite or controlling external resources.
pub(crate) fn observe_reconciliation_input(
    input: &ReconciliationInput,
    managed_caddy_directory: &Path,
) -> Result<Option<ReconciliationObservation>, ReconciliationReadError> {
    let Some(active) = &input.persisted.active else {
        return Ok(None);
    };
    let Some(runtime) = &active.runtime else {
        return Ok(None);
    };
    let recorded_container =
        observe_container(&runtime.external_runtime_id, runtime.container_port)
            .map_err(|source| ReconciliationReadError::ObserveContainer { source })?;
    let name = container_name(&input.desired.application.name, &active.deployment.id);
    let named_container = observe_named_container(&name, runtime.container_port)
        .map_err(|source| ReconciliationReadError::ObserveNamedContainer { source })?;
    let unit = unit_name(&input.desired.application.name, &active.deployment.id);
    let quadlet_source = observe_unit_source(&unit)
        .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    let systemd_unit = observe_generated_unit(&unit)
        .map_err(|source| ReconciliationReadError::ObserveQuadlet { source })?;
    let caddy_fragment =
        observe_caddy_fragment(managed_caddy_directory, &input.desired.application.id)
            .map_err(|source| ReconciliationReadError::ObserveCaddy { source })?;
    Ok(Some(ReconciliationObservation {
        recorded_container,
        named_container,
        quadlet_source,
        systemd_unit,
        caddy_fragment,
    }))
}
