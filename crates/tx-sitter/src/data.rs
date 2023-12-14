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
    pub status:  TxStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum TxStatus {
    Unsent,
    Pending,
    Mined,
    Finalized,
}
