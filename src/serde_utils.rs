use std::fmt;
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonStrWrapper<T>(pub T);

impl<T> FromStr for JsonStrWrapper<T>
where
    T: DeserializeOwned,
{
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map(JsonStrWrapper)
    }
}

impl<T> fmt::Display for JsonStrWrapper<T>
where
    T: Serialize,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = serde_json::to_string(self).map_err(|_| fmt::Error)?;

        s.fmt(f)
    }
}
