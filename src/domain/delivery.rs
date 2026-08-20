use serde::Deserialize;

use crate::domain::release::OciRepository;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryType {
    Oci,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Defines the immutable repository boundary allowed for application artifacts.
pub struct DeliverySpecification {
    delivery_type: DeliveryType,
    image_repository: OciRepository,
}

impl DeliverySpecification {
    pub fn new(delivery_type: DeliveryType, image_repository: OciRepository) -> Self {
        Self {
            delivery_type,
            image_repository,
        }
    }
    pub fn delivery_type(&self) -> DeliveryType {
        self.delivery_type
    }
    pub fn image_repository(&self) -> &OciRepository {
        &self.image_repository
    }
}
