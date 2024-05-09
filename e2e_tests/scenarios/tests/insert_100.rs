mod common;

use common::prelude::*;
use serde_json::Value;
use tokio::time::sleep;

use crate::common::docker_compose;

#[tokio::test]
async fn insert_100() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting e2e test");

    let docker_compose = docker_compose::setup("./../docker-compose").await?;

    let uri = format!("http://{}", docker_compose.get_local_addr());
    let client = Client::new();

    let identities = generate_test_commitments(100);

    for commitment in identities.iter() {
        _ = insert_identity_with_retries(&client, &uri, commitment, 10, 3.0).await?;
    }

    for commitment in identities.iter() {
        _ = mined_inclusion_proof_with_retries(&client, &uri, commitment, 60, 10.0).await?;
    }

    Ok(())
}

async fn insert_identity_with_retries(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &String,
    retries_count: usize,
    retries_interval: f32,
) -> anyhow::Result<()> {
    let mut last_res = Err(Error::msg("No calls at all"));
    for _i in 0..retries_count {
        last_res = insert_identity(&client, &uri, &commitment).await;

        if last_res.is_ok() {
            break;
        }

        _ = sleep(Duration::from_secs_f32(retries_interval)).await;
    }

    if let Err(err) = last_res {
        return Err(err);
    };

    last_res
}

async fn mined_inclusion_proof_with_retries(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &String,
    retries_count: usize,
    retries_interval: f32,
) -> anyhow::Result<Value> {
    let mut last_res = Err(Error::msg("No calls at all"));
    for _i in 0..retries_count {
        last_res = inclusion_proof(&client, &uri, &commitment).await;

        if let Ok(ref inclusion_proof_json) = last_res {
            if inclusion_proof_json["status"] == "mined" {
                break;
            }
        };

        _ = sleep(Duration::from_secs_f32(retries_interval)).await;
    }

    let inclusion_proof_json = last_res?;

    assert_eq!(inclusion_proof_json["status"], "mined");

    Ok(inclusion_proof_json)
}
