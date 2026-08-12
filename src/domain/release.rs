#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub id: String,
    pub application_id: String,
    pub image_reference: String,
    pub image_repository: String,
    pub image_digest: String,
    pub created_at: String,
}
