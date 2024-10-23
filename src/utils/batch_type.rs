use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, sqlx::Type, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[sqlx(type_name = "VARCHAR", rename_all = "PascalCase")]
pub enum BatchType {
    #[default]
    Insertion,
    Deletion,
}

impl BatchType {
    pub fn is_insertion(self) -> bool {
        self == Self::Insertion
    }

    pub fn is_deletion(self) -> bool {
        self == Self::Deletion
    }
}

impl From<String> for BatchType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "Insertion" => BatchType::Insertion,
            "Deletion" => BatchType::Deletion,
            _ => BatchType::Insertion,
        }
    }
}

impl std::fmt::Display for BatchType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BatchType::Insertion => write!(f, "insertion"),
            BatchType::Deletion => write!(f, "deletion"),
        }
    }
}
