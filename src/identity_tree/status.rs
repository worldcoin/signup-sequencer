use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The status pertains to the status of the root.
/// But it can also be used interchangeably with the status of an identity
/// as all identity commitments have an associated root.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Root is included in sequencer's in-memory tree, but is not yet
    /// mined.
    Pending,

    /// Root is mined on mainnet but is still waiting for confirmation on
    /// relayed chains
    ///
    /// i.e. the root is included in a mined block on mainnet,
    /// but the state has not yet been bridged to Optimism and Polygon
    ///
    /// NOTE: If the sequencer is not configured with any secondary chains this
    /// status should immediately become Finalized
    Processed,

    /// Root is mined and relayed to secondary chains
    Mined,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum UnprocessedStatus {
    /// Unprocessed identity failed to be inserted into the tree for some reason
    ///
    /// Usually accompanied by an appropriate error message
    Failed,

    /// Root is unprocessed - i.e. not included in sequencer's
    /// in-memory tree.
    New,
}

/// A status type visible on the API level - contains both the processed and
/// unprocessed statuses
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum ApiStatus {
    Unprocessed(UnprocessedStatus),
    Processed(Status),
}

#[derive(Debug, Error)]
#[error("unknown status")]
pub struct UnknownStatus;

impl FromStr for Status {
    type Err = UnknownStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "mined" => Ok(Self::Mined),
            "processed" => Ok(Self::Processed),
            _ => Err(UnknownStatus),
        }
    }
}

impl From<Status> for &str {
    fn from(scope: Status) -> Self {
        match scope {
            Status::Pending => "pending",
            Status::Mined => "mined",
            Status::Processed => "processed",
        }
    }
}

impl FromStr for ApiStatus {
    type Err = UnknownStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(s) = UnprocessedStatus::from_str(s) {
            Ok(Self::Unprocessed(s))
        } else if let Ok(s) = Status::from_str(s) {
            Ok(Self::Processed(s))
        } else {
            Err(UnknownStatus)
        }
    }
}

impl FromStr for UnprocessedStatus {
    type Err = UnknownStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "new" => Ok(Self::New),
            "failed" => Ok(Self::Failed),
            _ => Err(UnknownStatus),
        }
    }
}

impl From<UnprocessedStatus> for &str {
    fn from(scope: UnprocessedStatus) -> Self {
        match scope {
            UnprocessedStatus::New => "new",
            UnprocessedStatus::Failed => "failed",
        }
    }
}

impl From<UnprocessedStatus> for ApiStatus {
    fn from(status: UnprocessedStatus) -> Self {
        Self::Unprocessed(status)
    }
}

impl From<Status> for ApiStatus {
    fn from(status: Status) -> Self {
        Self::Processed(status)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(ApiStatus::Processed(Status::Pending) => "pending")]
    #[test_case(ApiStatus::Processed(Status::Mined) => "mined")]
    #[test_case(ApiStatus::Unprocessed(UnprocessedStatus::New) => "new")]
    #[test_case(ApiStatus::Unprocessed(UnprocessedStatus::Failed) => "failed")]
    fn serialize_status(api_status: ApiStatus) -> &'static str {
        let s = serde_json::to_string(&api_status).unwrap();

        let s = s.leak() as &'static str;

        // Unwrap from the redundant JSON quotes
        s.trim_start_matches("\"").trim_end_matches("\"")
    }

    #[test_case("pending" => ApiStatus::Processed(Status::Pending))]
    #[test_case("mined" => ApiStatus::Processed(Status::Mined))]
    #[test_case("new" => ApiStatus::Unprocessed(UnprocessedStatus::New))]
    #[test_case("failed" => ApiStatus::Unprocessed(UnprocessedStatus::Failed))]
    fn deserialize_status(s: &str) -> ApiStatus {
        // Wrapped because JSON expected `"something"` and not `something`
        let wrapped = format!("\"{s}\"");

        serde_json::from_str(&wrapped).unwrap()
    }
}
