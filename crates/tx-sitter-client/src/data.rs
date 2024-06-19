use ethers::types::{Address, Bytes, H256, U256};
use serde::{Deserialize, Serialize};
use strum::Display;

mod decimal_u256;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTxRequest {
    pub to:        Address,
    #[serde(with = "decimal_u256")]
    pub value:     U256,
    #[serde(default)]
    pub data:      Option<Bytes>,
    #[serde(with = "decimal_u256")]
    pub gas_limit: U256,
    #[serde(default)]
    pub priority:  TransactionPriority,
    #[serde(default)]
    pub tx_id:     Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
pub enum TransactionPriority {
    // 5th percentile
    Slowest = 0,
    // 25th percentile
    Slow    = 1,
    // 50th percentile
    #[default]
    Regular = 2,
    // 75th percentile
    Fast    = 3,
    // 95th percentile
    Fastest = 4,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTxResponse {
    pub tx_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTxResponse {
    pub tx_id:     String,
    pub to:        Address,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data:      Option<Bytes>,
    #[serde(with = "decimal_u256")]
    pub value:     U256,
    #[serde(with = "decimal_u256")]
    pub gas_limit: U256,
    pub nonce:     u64,

    // Sent tx data
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<H256>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status:  Option<TxStatus>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Display, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum TxStatus {
    Pending,
    Mined,
    Finalized,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_partial_tx_response() {
        const DATA: &str = indoc::indoc! {r#"{
                "data": "0xff",
                "gasLimit": "2000000",
                "nonce": 54,
                "status": null,
                "to": "0x928a514350a403e2f5e3288c102f6b1ccabeb37c",
                "txHash": null,
                "txId": "99e83a12-d6df-4f9c-aa43-048b38561dfd",
                "value": "0"
            }
        "#};

        serde_json::from_str::<GetTxResponse>(DATA).unwrap();
    }
}
