use std::time::Duration;

use anyhow::Error;
use hyper::client::HttpConnector;
use hyper::{Body, Client, Request};
use serde_json::{json, Value};
use signup_sequencer::identity_tree::Hash;
use signup_sequencer::server::data::{
    DeletionRequest, InclusionProofRequest, InclusionProofResponse, InsertCommitmentRequest,
};
use tracing::debug;

use crate::common::prelude::StatusCode;

pub struct RawResponse {
    pub status_code: StatusCode,
    pub body: String,
}

pub async fn insert_identity(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &Hash,
) -> anyhow::Result<()> {
    debug!("Calling /insertIdentity");
    let body = Body::from(serde_json::to_string(&InsertCommitmentRequest {
        identity_commitment: *commitment,
    })?);

    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/insertIdentity")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create insert identity hyper::Body");

    let mut response = client
        .request(req)
        .await
        .expect("Failed to execute request.");
    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    if !response.status().is_success() {
        return Err(Error::msg(format!(
            "Failed to insert identity: response = {}",
            response.status()
        )));
    }

    assert!(bytes.is_empty());

    Ok(())
}

pub async fn delete_identity(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &Hash,
) -> anyhow::Result<()> {
    debug!("Calling /deleteIdentity");
    let body = Body::from(serde_json::to_string(&DeletionRequest {
        identity_commitment: *commitment,
    })?);

    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/deleteIdentity")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create delete identity hyper::Body");

    let mut response = client
        .request(req)
        .await
        .expect("Failed to execute request.");
    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    if !response.status().is_success() {
        return Err(Error::msg(format!(
            "Failed to delete identity: response = {}",
            response.status()
        )));
    }

    assert!(bytes.is_empty());

    Ok(())
}

pub async fn inclusion_proof_raw(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &Hash,
) -> anyhow::Result<RawResponse> {
    debug!("Calling /inclusionProof");
    let body = Body::from(serde_json::to_string(&InclusionProofRequest {
        identity_commitment: *commitment,
    })?);

    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/inclusionProof")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create inclusion proof hyper::Body");

    let mut response = client
        .request(req)
        .await
        .expect("Failed to execute request.");
    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    let result = String::from_utf8(bytes.into_iter().collect())
        .expect("Could not parse response bytes to utf-8");

    let raw_response = RawResponse {
        status_code: response.status(),
        body: result,
    };

    debug!(
        "Response status={}, body={}",
        raw_response.status_code, raw_response.body
    );

    Ok(raw_response)
}

pub async fn inclusion_proof(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &Hash,
) -> anyhow::Result<Option<InclusionProofResponse>> {
    let result = inclusion_proof_raw(client, uri, commitment).await?;

    if result.status_code == StatusCode::NOT_FOUND {
        return Ok(None);
    }

    let result_json = serde_json::from_str::<InclusionProofResponse>(&result.body)
        .expect("Failed to parse response as json");

    Ok(Some(result_json))
}
