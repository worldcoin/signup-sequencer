use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The status pertains to the status of the root.
/// But it can also be used interchangeably with the status of an identity
/// as all identity commitments have an associated root.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ProcessedStatus {
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

/// Status of identity commitments which have not yet been included in the tree
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
pub enum Status {
    Unprocessed(UnprocessedStatus),
    Processed(ProcessedStatus),
}

#[derive(Debug, Error)]
#[error("unknown status")]
pub struct UnknownStatus;

impl FromStr for ProcessedStatus {
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

impl From<ProcessedStatus> for &str {
    fn from(scope: ProcessedStatus) -> Self {
        match scope {
            ProcessedStatus::Pending => "pending",
            ProcessedStatus::Mined => "mined",
            ProcessedStatus::Processed => "processed",
        }
    }
}

impl FromStr for Status {
    type Err = UnknownStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(s) = UnprocessedStatus::from_str(s) {
            Ok(Self::Unprocessed(s))
        } else if let Ok(s) = ProcessedStatus::from_str(s) {
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

impl From<UnprocessedStatus> for Status {
    fn from(status: UnprocessedStatus) -> Self {
        Self::Unprocessed(status)
    }
}

impl From<ProcessedStatus> for Status {
    fn from(status: ProcessedStatus) -> Self {
        Self::Processed(status)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(Status::Processed(ProcessedStatus::Pending) => "pending")]
    #[test_case(Status::Processed(ProcessedStatus::Mined) => "mined")]
    #[test_case(Status::Unprocessed(UnprocessedStatus::New) => "new")]
    #[test_case(Status::Unprocessed(UnprocessedStatus::Failed) => "failed")]
    fn serialize_status(api_status: Status) -> &'static str {
        let s = serde_json::to_string(&api_status).unwrap();

        let s = s.leak() as &'static str;

        // Unwrap from the redundant JSON quotes
        s.trim_start_matches("\"").trim_end_matches("\"")
    }

    #[test_case("pending" => Status::Processed(ProcessedStatus::Pending))]
    #[test_case("mined" => Status::Processed(ProcessedStatus::Mined))]
    #[test_case("new" => Status::Unprocessed(UnprocessedStatus::New))]
    #[test_case("failed" => Status::Unprocessed(UnprocessedStatus::Failed))]
    fn deserialize_status(s: &str) -> Status {
        // Wrapped because JSON expected `"something"` and not `something`
        let wrapped = format!("\"{s}\"");

        serde_json::from_str(&wrapped).unwrap()
    }
}
