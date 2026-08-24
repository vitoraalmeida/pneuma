use crate::domain::application::ApplicationName;
use crate::domain::exposure::ExposureIntent;
use crate::domain::release::{DeliveryType, OciRepository};
use crate::domain::runtime::{ContainerPort, HealthCheckPath, HealthCheckStatus};
use crate::domain::system::SystemName;

#[derive(Debug, Clone, PartialEq, Eq)]
// Carries manifest values whose invariants were checked once at the manifest
// boundary, ready for the import workflow. This is a use-case input type: it is
// deliberately free of TOML layout so the domain never depends on the external
// file schema.
pub struct ImportSpecification {
    // Intentional primitive: the schema version is compared once for equality
    // against the supported version at the manifest boundary (never incremented,
    // persisted as a snapshot copy on the Application); a dedicated type would
    // have nothing to own beyond that single check.
    pub schema_version: u32,
    pub system_name: Option<SystemName>,
    pub application_name: ApplicationName,
    pub delivery_type: DeliveryType,
    pub repository: OciRepository,
    pub container_port: ContainerPort,
    pub healthcheck_path: HealthCheckPath,
    pub expected_status: HealthCheckStatus,
    pub exposure: ExposureIntent,
}
