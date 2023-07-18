//! Transactions as defined by the OpenZeppelin Defender API.
//!
//! https://docs.openzeppelin.com/defender/relay-api-reference#txs-endpoint

use std::fmt;

use chrono::{DateTime, Utc};
use ethers::types::{Bytes, NameOrAddress, H256, U256};
use serde::{Deserialize, Serialize};

/// OpenZeppelin Defender transaction status.
///
/// https://docs.openzeppelin.com/defender/relay-api-reference#transaction-status
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Pending,
    Sent,
    Submitted,
    Inmempool,
    Mined,
    Confirmed,
    Failed,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Status::Pending => write!(f, "pending"),
            Status::Sent => write!(f, "sent"),
            Status::Submitted => write!(f, "submitted"),
            Status::Inmempool => write!(f, "inmempool"),
            Status::Mined => write!(f, "mined"),
            Status::Confirmed => write!(f, "confirmed"),
            Status::Failed => write!(f, "failed"),
        }
    }
}

/// OpenZeppelin Defender transaction to be sent.
///
/// https://docs.openzeppelin.com/defender/relay-api-reference#send-transaction
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendBaseTransactionRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to:          Option<&'a NameOrAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value:       Option<&'a U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data:        Option<&'a Bytes>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_limit:   Option<&'a U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<DateTime<Utc>>,
}

/// OpenZeppelin Defender transaction to be sent.
///
/// https://docs.openzeppelin.com/defender/relay-api-reference#send-transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendBaseTransactionRequestOwned {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub to:          Option<NameOrAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub value:       Option<U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub data:        Option<Bytes>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub gas_limit:   Option<U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub valid_until: Option<DateTime<Utc>>,
}

/// OpenZeppelin Defender transaction that has been received by the relayer and
/// can be queried for.
///
/// https://docs.openzeppelin.com/defender/relay-api-reference#txs-endpoint
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayerTransactionBase {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub hash:           Option<H256>,
    pub transaction_id: String,
    pub to:             NameOrAddress,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub value:          Option<U256>,
    pub gas_limit:      u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub data:           Option<Bytes>,
    pub valid_until:    DateTime<Utc>,
    pub status:         Status,
}
