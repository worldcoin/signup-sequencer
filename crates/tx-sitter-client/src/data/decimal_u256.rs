use ethers::types::U256;

pub fn serialize<S>(u256: &U256, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let s = u256.to_string();
    serializer.serialize_str(&s)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<U256, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: &str = serde::Deserialize::deserialize(deserializer)?;
    let u256 = U256::from_dec_str(s).map_err(serde::de::Error::custom)?;
    Ok(u256)
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Test {
        #[serde(with = "super")]
        v: U256,
    }

    #[test]
    fn test_u256_serde() {
        let test = Test { v: U256::from(123) };

        let s = serde_json::to_string(&test).unwrap();
        assert_eq!(s, r#"{"v":"123"}"#);

        let test: Test = serde_json::from_str(&s).unwrap();
        assert_eq!(test.v, U256::from(123));
    }
}
