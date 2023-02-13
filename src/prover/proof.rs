use ethers::types::U256;
use serde::{Deserialize, Serialize};

/// The proof term returned from the `semaphore-mtb` proof generation service.
///
/// The names of the data fields match those from the JSON response exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    pub ar:  [U256; 2],
    pub bs:  [[U256; 2]; 2],
    pub krs: [U256; 2],
}

impl From<[U256; 8]> for Proof {
    fn from(value: [U256; 8]) -> Self {
        Self {
            ar:  [value[0], value[1]],
            bs:  [[value[2], value[3]], [value[4], value[5]]],
            krs: [value[6], value[7]],
        }
    }
}

impl From<Proof> for [U256; 8] {
    fn from(value: Proof) -> Self {
        [
            value.ar[0],
            value.ar[1],
            value.bs[0][0],
            value.bs[0][1],
            value.bs[1][0],
            value.bs[1][1],
            value.krs[0],
            value.krs[1],
        ]
    }
}
