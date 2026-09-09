use std::time::Duration;

use anyhow::Error;
use reqwest::Client;
use serde_json::{json, Value};
use signup_sequencer::identity_tree::Hash;
use signup_sequencer::server::api_v1::data::{
    DeletionRequest, InclusionProofRequest, InclusionProofResponse, InsertCommitmentRequest,
};
use tracing::{debug, info};

use crate::common::prelude::StatusCode;

pub struct RawResponse {
    pub status_code: StatusCode,
    pub body: String,
}

pub async fn insert_identity(
    client: &Client,
    uri: &String,
    commitment: &Hash,
) -> anyhow::Result<()> {
    debug!("Calling /insertIdentity");
    let body = serde_json::to_string(&InsertCommitmentRequest {
        identity_commitment: *commitment,
    })?;

    let response = client
        .post(uri.to_owned() + "/insertIdentity")
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("Failed to execute request.");
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .expect("Failed to convert response body to bytes");
    if !status.is_success() {
        return Err(Error::msg(format!(
            "Failed to insert identity: response = {status}"
        )));
    }

    assert!(bytes.is_empty());

    Ok(())
}

pub async fn delete_identity(
    client: &Client,
    uri: &String,
    commitment: &Hash,
) -> anyhow::Result<()> {
    debug!("Calling /deleteIdentity");
    let body = serde_json::to_string(&DeletionRequest {
        identity_commitment: *commitment,
    })?;

    let response = client
        .post(uri.to_owned() + "/deleteIdentity")
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("Failed to execute request.");
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .expect("Failed to convert response body to bytes");
    if !status.is_success() {
        return Err(Error::msg(format!(
            "Failed to delete identity: response = {status}"
        )));
    }

    assert!(bytes.is_empty());

    Ok(())
}

pub async fn inclusion_proof_raw(
    client: &Client,
    uri: &String,
    commitment: &Hash,
) -> anyhow::Result<RawResponse> {
    debug!("Calling /inclusionProof");
    let body = serde_json::to_string(&InclusionProofRequest {
        identity_commitment: *commitment,
    })?;

    let response = client
        .post(uri.to_owned() + "/inclusionProof")
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("Failed to execute request.");
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .expect("Failed to convert response body to bytes");
    let result = String::from_utf8(bytes.into_iter().collect())
        .expect("Could not parse response bytes to utf-8");

    let raw_response = RawResponse {
        status_code: status,
        body: result,
    };

    debug!(
        "Response status={}, body={}",
        raw_response.status_code, raw_response.body
    );

    Ok(raw_response)
}

pub async fn inclusion_proof(
    client: &Client,
    uri: &String,
    commitment: &Hash,
) -> anyhow::Result<(StatusCode, Option<InclusionProofResponse>)> {
    let result = inclusion_proof_raw(client, uri, commitment).await?;

    if !result.status_code.is_success() {
        return Ok((result.status_code, None));
    }

    let result_json = serde_json::from_str::<InclusionProofResponse>(&result.body)
        .expect("Failed to parse response as json");

    Ok((result.status_code, Some(result_json)))
}
