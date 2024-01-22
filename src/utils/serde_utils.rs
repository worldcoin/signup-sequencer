use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
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

impl<T> Serialize for JsonStrWrapper<T>
where
    T: Serialize,
{
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde_json::to_string(&self.0)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for JsonStrWrapper<T>
where
    // TODO: Is there some way to use T:
    // Deserialize<'de> here?
    T: DeserializeOwned,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'static, str>::deserialize(deserializer)?;

        serde_json::from_str(&s)
            .map(JsonStrWrapper)
            .map_err(serde::de::Error::custom)
    }
}

impl<T> From<T> for JsonStrWrapper<T> {
    fn from(t: T) -> Self {
        Self(t)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn json() {
        let wrapper = JsonStrWrapper(vec![1, 2, 3]);

        let s = serde_json::to_string(&wrapper).unwrap();

        assert_eq!(s, "\"[1,2,3]\"");

        let wrapper: JsonStrWrapper<Vec<u32>> = serde_json::from_str(&s).unwrap();

        assert_eq!(wrapper.0, vec![1, 2, 3]);
    }

    #[test]
    fn json_value() {
        let wrapper = JsonStrWrapper(vec![1, 2, 3]);

        let s = serde_json::to_value(wrapper).unwrap();

        assert_eq!(s, Value::String("[1,2,3]".to_string()));

        let wrapper: JsonStrWrapper<Vec<u32>> = serde_json::from_value(s).unwrap();

        assert_eq!(wrapper.0, vec![1, 2, 3]);
    }
}
