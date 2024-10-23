use crate::utils::batch_type::BatchType;

use super::App;

impl App {
    pub async fn determine_next_batch_type(&self) -> BatchType {
        BatchType::Insertion
    }

    pub async fn create_batch() {

    }
}
