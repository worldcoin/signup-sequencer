use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Default,
    // sqlx
    sqlx::Type,
    // serde
    Serialize,
    Deserialize,
    // strum
    EnumString,
    Display,
)]
#[serde(rename_all = "PascalCase")]
#[sqlx(type_name = "VARCHAR", rename_all = "PascalCase")]
#[strum(serialize_all = "PascalCase")]
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn display() {
        assert_eq!(BatchType::Insertion.to_string(), "Insertion");
        assert_eq!(BatchType::Deletion.to_string(), "Deletion");
    }

    #[test]
    fn from_str() {
        assert_eq!(
            BatchType::from_str("Insertion").unwrap(),
            BatchType::Insertion
        );
        assert_eq!(
            BatchType::from_str("Deletion").unwrap(),
            BatchType::Deletion
        );
        assert!(BatchType::from_str("Unknown").is_err());
    }

    #[test]
    fn serialize() {
        let insertion = serde_json::to_string(&BatchType::Insertion).unwrap();
        let deletion = serde_json::to_string(&BatchType::Deletion).unwrap();

        assert_eq!(insertion, "\"Insertion\"");
        assert_eq!(deletion, "\"Deletion\"");
    }
}
