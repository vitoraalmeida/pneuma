use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryType {
    Oci,
}

impl DeliveryType {
    pub fn database_value(self) -> &'static str {
        match self {
            Self::Oci => "oci",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "oci" => Some(Self::Oci),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverySpecification {
    pub delivery_type: DeliveryType,
    pub image_repository: String,
}
