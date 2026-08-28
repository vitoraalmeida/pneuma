use crate::domain::application::ApplicationName;
use crate::domain::exposure::ExposureIntent;
use crate::domain::release::DeliverySpecification;
use crate::domain::runtime::RuntimeSpecification;
use crate::domain::system::SystemName;

#[derive(Debug, Clone, PartialEq, Eq)]
// Carries manifest values whose invariants were checked once at the manifest
// boundary, ready for the import workflow. Delivery and runtime intent reuse
// the canonical aggregates persisted for deployments, so import never keeps a
// parallel field-by-field representation. This is a use-case input type: it is
// deliberately free of TOML layout so the domain never depends on the external
// file schema.
pub struct ImportSpecification {
    pub system_name: Option<SystemName>,
    pub application_name: ApplicationName,
    pub delivery: DeliverySpecification,
    pub runtime: RuntimeSpecification,
    pub exposure: ExposureIntent,
}
