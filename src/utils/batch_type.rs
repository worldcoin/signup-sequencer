#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BatchType {
    Insertion,
    Deletion,
}

impl BatchType {
    pub fn is_deletion(self) -> bool {
        self == Self::Deletion
    }
}
