use ethers::types::U256;
use serde::{de::Error as _, ser::Error as _, Deserialize, Serialize};
use std::{
    fmt::Debug,
    str::{from_utf8, FromStr},
};

/// Container for 256-bit hash values.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Hash([u8; 32]);

impl Hash {
    pub const fn from_bytes_be(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes_be(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Debug print hashes using `hex!(..)` literals.
impl Debug for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hash(hex!(\"{}\"))", hex::encode(&self.0))
    }
}

/// Conversion from Ether U256
impl From<&Hash> for U256 {
    fn from(hash: &Hash) -> Self {
        Self::from_big_endian(hash.as_bytes_be())
    }
}

/// Conversion to Ether U256
impl From<U256> for Hash {
    fn from(u256: U256) -> Self {
        let mut bytes = [0_u8; 32];
        u256.to_big_endian(&mut bytes);
        Self::from_bytes_be(bytes)
    }
}

/// Parse Hash from hex string.
/// Hex strings can be upper/lower/mixed case and have an optional `0x` prefix
/// but they must always be exactly 32 bytes.
impl FromStr for Hash {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let str = trim_hex_prefix(s);
        let mut out = [0_u8; 32];
        hex::decode_to_slice(str, &mut out)?;
        Ok(Self(out))
    }
}

/// Serialize hashes into human readable hex strings or byte arrays.
/// Hex strings are lower case without prefix and always 32 bytes.
impl Serialize for Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if serializer.is_human_readable() {
            let mut hex_ascii = [0_u8; 64];
            hex::encode_to_slice(self.0, &mut hex_ascii)
                .map_err(|e| S::Error::custom(format!("Error hex encoding: {}", e)))?;
            from_utf8(&hex_ascii)
                .map_err(|e| S::Error::custom(format!("Invalid hex encoding: {}", e)))?
                .serialize(serializer)
        } else {
            self.0.serialize(serializer)
        }
    }
}

/// Deserialize human readable hex strings or byte arrays into hashes.
/// Hex strings can be upper/lower/mixed case and have an optional `0x` prefix
/// but they must always be exactly 32 bytes.
impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let str = <&'de str>::deserialize(deserializer)?;
            Self::from_str(str).map_err(|e| D::Error::custom(format!("Error in hex: {}", e)))
        } else {
            <[u8; 32]>::deserialize(deserializer).map(Hash)
        }
    }
}

/// Helper function to optionally remove `0x` prefix from hex strings.
fn trim_hex_prefix(str: &str) -> &str {
    if str.len() >= 2 && (&str[..2] == "0x" || &str[..2] == "0X") {
        &str[2..]
    } else {
        str
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use hex_literal::hex;
    use proptest::proptest;
    use serde_json::{from_str, to_string};

    #[test]
    fn test_serialize() {
        let hash = Hash([0; 32]);
        assert_eq!(
            to_string(&hash).unwrap(),
            "\"0000000000000000000000000000000000000000000000000000000000000000\""
        );
        let hash = Hash(hex!(
            "1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe"
        ));
        assert_eq!(
            to_string(&hash).unwrap(),
            "\"1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe\""
        );
    }

    #[test]
    fn test_deserialize() {
        assert_eq!(
            from_str::<Hash>(
                "\"0x1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe\""
            )
            .unwrap(),
            Hash(hex!(
                "1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe"
            ))
        );
        assert_eq!(
            from_str::<Hash>(
                "\"0X1C4823575d154474EE3e5ac838d002456a815181437afd14f126da58a9912bbe\""
            )
            .unwrap(),
            Hash(hex!(
                "1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe"
            ))
        );
    }

    #[test]
    fn test_roundtrip() {
        proptest!(|(bytes: [u8; 32])| {
            let hash = Hash(bytes);
            let json = to_string(&hash).unwrap();
            let parsed = from_str(&json).unwrap();
            assert_eq!(hash, parsed);
        });
    }
}
